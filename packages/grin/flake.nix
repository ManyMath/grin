{
  description = "Reproducible reference builds of grin's Rust library (grin-ffi)";

  inputs = {
    # The build itself lives in the kit; it also pins nixpkgs and the Rust
    # and zig overlays, and this flake follows those pins. flake.lock pins
    # the kit to a commit; pubspec.yaml pins the hook package to the same
    # one (test/kit_pin_test.dart checks that they agree).
    kit.url = "git+ssh://git@git.cypherstack.com:222/sneurlax/native-build-kit.git?ref=feat/native-build-kit";
    nixpkgs.follows = "kit/nixpkgs";
    rust-overlay.follows = "kit/rust-overlay";
    zig-overlay.follows = "kit/zig-overlay";

    # The grin-wallet fork is a private repository. A sandboxed fetchgit
    # (what importCargoLock would use for a git dependency) has no ssh
    # credentials, so the fork is a flake input instead: Nix fetches it at
    # evaluation time with the user's ssh setup, flake.lock pins it by rev
    # and narHash, and the overlay below hands that tree to importCargoLock
    # in place of fetchgit. Keep rev equal to the rev in
    # rust/Cargo.toml (test/kit_pin_test.dart checks that too).
    grin-wallet = {
      url = "git+ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git?ref=optional-tor-rustls&rev=64d361f2e49fa6d544cf0236417da48a99444b2f&submodules=1";
      flake = false;
    };
  };

  outputs = { self, kit, grin-wallet, ... }:
    let
      lib = kit.inputs.nixpkgs.lib;
      settings = builtins.fromJSON (builtins.readFile ./build-config.json);

      # build-config.json and rust/rust-toolchain.toml both name the Rust
      # version: Nix builds with the first, rustup (cargo mode, local cargo
      # runs) with the second. Evaluation fails when they disagree, or when
      # the toolchain file lacks a target the flake builds.
      toolchain = (builtins.fromTOML (builtins.readFile ./rust/rust-toolchain.toml)).toolchain;
      missingTargets = builtins.filter (t: !(builtins.elem t toolchain.targets)) settings.targets;
      checkedSettings =
        if toolchain.channel != settings.rust then
          throw "build-config.json pins Rust ${settings.rust}, rust/rust-toolchain.toml ${toolchain.channel}; make them agree"
        else if missingTargets != [ ] then
          throw "rust/rust-toolchain.toml does not list ${builtins.concatStringsSep ", " missingTargets}"
        else settings;

      forkUrl = "ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git";

      # The rev Cargo.toml and Cargo.lock pin must be the rev this flake
      # fetched, or the vendored tree would not match the lock.
      lockedForkRev =
        let
          lock = builtins.fromTOML (builtins.readFile ./rust/Cargo.lock);
          impls = lib.findFirst (p: p.name == "grin_wallet_impls") (throw "grin_wallet_impls missing from Cargo.lock") lock.package;
          m = builtins.match ".*#([0-9a-f]+)$" impls.source;
        in
        if m == null then throw "grin_wallet_impls is not a git dependency in Cargo.lock" else builtins.head m;
      checkedFork =
        if grin-wallet.rev != lockedForkRev then
          throw "flake input grin-wallet is at ${grin-wallet.rev}, Cargo.lock pins ${lockedForkRev}; make them agree"
        else grin-wallet;

      # importCargoLock vendors every git dependency with fetchgit, keyed by
      # repository URL. Answer the fork's URL with the flake input; every
      # other call goes to the real fetchgit.
      forkOverlay = final: prev:
        let
          wrapped = args:
            if builtins.isAttrs args && (args.url or "") == forkUrl
            then checkedFork
            else prev.fetchgit args;
        in
        {
          # fetchgit is an overridable function (a set with __functor), and
          # other packages read its attributes, so keep them.
          fetchgit =
            if builtins.isAttrs prev.fetchgit
            then prev.fetchgit // { __functor = _: wrapped; }
            else wrapped;
        };

      forSystem = system: kit.lib.mkCargoLibrary {
        inherit system;
        pkgs = (kit.lib.pkgsFor system).extend forkOverlay;
        settings = checkedSettings;
        crate = "grin-ffi";
        src = ./rust;
        srcName = "rust";
        libraryBaseName = "grin_ffi";
        appleInstallName = "@rpath/libgrin_ffi.dylib";
        # Dynamic libraries only, the ones the hook registers and releases
        # publish.
        outputNames = {
          linux = [ "libgrin_ffi.so" ];
          android = [ "libgrin_ffi.so" ];
          windows = [ "grin_ffi.dll" ];
          apple = [ "libgrin_ffi.dylib" ];
        };
        # The hash importCargoLock expects for the fork's commit. The
        # overlay above answers that fetch with the flake input, so the
        # value is informational; it is the input's own narHash.
        cargoLock.outputHashes = {
          "grin_wallet_impls-5.5.1" = checkedFork.narHash;
        };
        # Fail an Android build whose library needs a symbol API 24 lacks.
        androidSymbolCheck = true;
        families.android = {
          # 16 KB pages, as Stack Wallet's other plugins link.
          extraRustflags = [ "-Clink-arg=-Wl,-z,max-page-size=16384" ];
        };
        # lmdb-rs's C build on Apple: the same flags the pinned legacy hooks
        # used for mobile LMDB (POSIX mutexes without robust-mutex support).
        families.apple = {
          extraCflags = [ "-DMDB_USE_POSIX_MUTEX=1" "-DMDB_USE_ROBUST=0" ];
        };
      };
      systems = kit.lib.defaultSystems;
      perSystem = lib.genAttrs systems forSystem;
    in
    {
      packages = lib.mapAttrs (_: l: l.packages) perSystem;
      devShells = lib.mapAttrs (_: l: l.devShells) perSystem;
    };
}
