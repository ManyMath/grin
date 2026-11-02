//! Error codes, the result struct every entry point returns, the panic guard
//! and helpers for reading C string arguments.

use std::any::Any;
use std::ffi::{CStr, CString};
use std::fmt;
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use grin_util::ZeroingString;
use grin_wallet_libwallet::Error as LibwalletError;
use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroize;

/// Result of every fallible entry point.
///
/// On success `code` is 0, `error` is NULL and `data` is a JSON document (or
/// NULL when the function returns nothing). On failure `data` is NULL and
/// `error` is a JSON object `{"code", "message", "details"}`. Free it with
/// `grin_ffi_result_free` exactly once.
#[repr(C)]
pub struct GrinFfiResult {
    pub code: i32,
    pub data: *mut c_char,
    pub error: *mut c_char,
}

/// Error codes. Dart maps these by value, so variants are append-only.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrinErrorCode {
    Ok = 0,
    InvalidArgument = 1,
    InvalidUtf8 = 2,
    Json = 3,
    WalletNotFound = 4,
    InvalidPassword = 5,
    WalletExists = 6,
    InvalidHandle = 7,
    Lifecycle = 8,
    Node = 9,
    InsufficientFunds = 10,
    Slate = 11,
    Transaction = 12,
    NotFound = 13,
    Cancelled = 14,
    Io = 15,
    InternalPanic = 16,
    Internal = 17,
    WalletBusy = 18,
    Http = 19,
    NotSupported = 20,
    InvalidAddress = 21,
    InvalidMnemonic = 22,
}

/// Error carried through the crate and serialized into `GrinFfiResult.error`.
#[derive(Clone, Debug)]
pub struct GrinError {
    pub code: GrinErrorCode,
    pub message: String,
    pub details: Option<Value>,
}

impl GrinError {
    pub fn new(code: GrinErrorCode, message: impl Into<String>) -> Self {
        GrinError {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(GrinErrorCode::InvalidArgument, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(GrinErrorCode::Internal, message)
    }

    pub fn not_supported() -> Self {
        Self::new(GrinErrorCode::NotSupported, "not implemented yet")
    }

    /// JSON text placed in `GrinFfiResult.error`.
    pub fn to_json(&self) -> String {
        #[derive(Serialize)]
        struct Out<'a> {
            code: i32,
            message: &'a str,
            details: &'a Option<Value>,
        }
        let out = Out {
            code: self.code as i32,
            message: &self.message,
            details: &self.details,
        };
        serde_json::to_string(&out).unwrap_or_else(|_| {
            format!(
                "{{\"code\":{},\"message\":\"error could not be serialized\",\"details\":null}}",
                self.code as i32
            )
        })
    }
}

impl fmt::Display for GrinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for GrinError {}

impl From<LibwalletError> for GrinError {
    fn from(e: LibwalletError) -> Self {
        use GrinErrorCode as C;
        let message = e.to_string();
        let code = match &e {
            LibwalletError::NotEnoughFunds {
                available, needed, ..
            } => {
                return GrinError::new(C::InsufficientFunds, message).with_details(
                    serde_json::json!({ "required": needed, "available": available }),
                );
            }
            LibwalletError::InvalidAmount
            | LibwalletError::BigAmountError(..)
            | LibwalletError::Fee(_) => C::Transaction,
            LibwalletError::WalletSeedExists(_) => C::WalletExists,
            LibwalletError::WalletSeedDoesntExist => C::WalletNotFound,
            LibwalletError::WalletSeedDecryption => C::InvalidPassword,
            // DefaultLCProvider reports a seed that fails to decrypt as a
            // plain Lifecycle error. The entry points that can meet one
            // (open, get_mnemonic, change_password) recognise its exact
            // message and check the seed file themselves, see
            // lifecycle::seed_open_error.
            LibwalletError::Lifecycle(_)
            | LibwalletError::KeychainDoesntExist
            | LibwalletError::InvalidKeychainMask => C::Lifecycle,
            // owner::cancel_tx refuses to cancel when it cannot reach the node.
            LibwalletError::Node
            | LibwalletError::ClientCallback(_)
            | LibwalletError::TransactionCancellationError(_) => C::Node,
            LibwalletError::WalletComms(_) | LibwalletError::Hyper | LibwalletError::Uri => C::Http,
            LibwalletError::IO(_) => C::Io,
            LibwalletError::Format(_) => C::Json,
            LibwalletError::SlateVersionParse
            | LibwalletError::SlateSer
            | LibwalletError::SlateDeser
            | LibwalletError::SlateState
            | LibwalletError::SlatepackSer
            | LibwalletError::SlatepackDeser(_)
            | LibwalletError::SlateVersion(_)
            | LibwalletError::SlateTransactionRequired
            | LibwalletError::SlateInvalidDowngrade(_)
            | LibwalletError::Compatibility(_)
            | LibwalletError::InvalidSlatepackData(_)
            | LibwalletError::SlatepackEncryption(_)
            | LibwalletError::SlatepackDecryption(_)
            | LibwalletError::Age(_) => C::Slate,
            LibwalletError::TransactionDoesntExist(_) => C::NotFound,
            LibwalletError::DuplicateTransactionId
            | LibwalletError::TransactionNotCancellable(_)
            | LibwalletError::TransactionDumpError(_)
            | LibwalletError::TransactionAlreadyConfirmed
            | LibwalletError::TransactionAlreadyReceived(_)
            | LibwalletError::TransactionWasCancelled(_)
            | LibwalletError::TransactionBuildingNotCompleted(_)
            | LibwalletError::TransactionExpired
            | LibwalletError::Transaction(_)
            | LibwalletError::LibTX(_)
            | LibwalletError::StoredTx(_)
            // A proof that does not verify (signature, kernel not on chain).
            | LibwalletError::PaymentProof(_) => C::Transaction,
            LibwalletError::SlatepackAddress(_)
            | LibwalletError::AddressDecoding(_)
            | LibwalletError::OnionV3Address(_)
            | LibwalletError::ED25519Key(_) => C::InvalidAddress,
            LibwalletError::PaymentProofRetrieval(_) => C::NotFound,
            // grin_store's NotFoundErr, e.g. no transaction context for a
            // slate that is unknown or already finalized.
            LibwalletError::Backend(msg) if msg.starts_with("DB Not Found Error") => C::NotFound,
            // Coin selection with the fee taken out of a too small amount.
            LibwalletError::GenericError(msg) if msg.contains("too small to include fee") => {
                C::Transaction
            }
            LibwalletError::PaymentProofParsing(_) => C::Json,
            LibwalletError::UnknownAccountLabel(_) => C::InvalidArgument,
            _ => C::Internal,
        };
        GrinError::new(code, message)
    }
}

impl From<grin_wallet_impls::Error> for GrinError {
    fn from(e: grin_wallet_impls::Error) -> Self {
        use grin_wallet_impls::Error as E;
        use GrinErrorCode as C;
        let message = e.to_string();
        let code = match e {
            E::LibWallet(inner) => return inner.into(),
            E::WalletSeedExists(_) => C::WalletExists,
            E::WalletSeedDoesntExist | E::WalletDoesntExist(..) => C::WalletNotFound,
            E::Encryption => C::InvalidPassword,
            E::Mnemonic(_) => C::InvalidMnemonic,
            E::ArgumentError(_) => C::InvalidArgument,
            E::OnionV3Address(_) | E::NotOnion(_) => C::InvalidAddress,
            E::IO => C::Io,
            E::Format => C::Json,
            _ => C::Internal,
        };
        GrinError::new(code, message)
    }
}

impl From<serde_json::Error> for GrinError {
    fn from(e: serde_json::Error) -> Self {
        GrinError::new(GrinErrorCode::Json, e.to_string())
    }
}

impl From<std::io::Error> for GrinError {
    fn from(e: std::io::Error) -> Self {
        GrinError::new(GrinErrorCode::Io, e.to_string())
    }
}

/// What every guarded body returns: a JSON payload, nothing, or an error.
pub type FfiOutcome = Result<Option<String>, GrinError>;

/// Runs an entry point body inside `catch_unwind`. Use through the macro so
/// the closure gets the right return type annotation.
macro_rules! ffi_guard {
    ($($body:tt)*) => {
        $crate::error::guard(move || -> $crate::error::FfiOutcome { $($body)* })
    };
}

pub fn guard<F>(f: F) -> GrinFfiResult
where
    F: FnOnce() -> FfiOutcome,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(data)) => GrinFfiResult::ok(data),
        Ok(Err(e)) => GrinFfiResult::err(&e),
        Err(payload) => {
            let message = panic_message(payload.as_ref());
            drop_payload(payload);
            GrinFfiResult::err(&GrinError::new(GrinErrorCode::InternalPanic, message))
        }
    }
}

/// Drops a panic payload without letting a panicking `Drop` unwind into an
/// `extern "C"` frame (which aborts the process). A payload whose drop
/// panics again is leaked instead.
fn drop_payload(payload: Box<dyn Any + Send>) {
    if let Err(again) = catch_unwind(AssertUnwindSafe(move || drop(payload))) {
        std::mem::forget(again);
    }
}

/// For the places outside `ffi_guard!` that catch a panic and cannot report
/// it to the caller (the void entry points): logs the message and drops the
/// payload safely.
pub(crate) fn log_caught_panic(context: &str, payload: Box<dyn Any + Send>) {
    log::error!("{context} panicked: {}", panic_message(payload.as_ref()));
    drop_payload(payload);
}

pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with a non-string payload".to_owned()
    }
}

impl GrinFfiResult {
    fn ok(data: Option<String>) -> Self {
        let data_ptr = match data {
            None => std::ptr::null_mut(),
            Some(s) => match CString::new(s) {
                Ok(c) => c.into_raw(),
                Err(e) => {
                    let mut bytes = e.into_vec();
                    bytes.zeroize();
                    return Self::err(&GrinError::internal(
                        "result contained an interior NUL byte",
                    ));
                }
            },
        };
        GrinFfiResult {
            code: GrinErrorCode::Ok as i32,
            data: data_ptr,
            error: std::ptr::null_mut(),
        }
    }

    fn err(e: &GrinError) -> Self {
        let code = if e.code == GrinErrorCode::Ok {
            GrinErrorCode::Internal
        } else {
            e.code
        };
        let json = if code == e.code {
            e.to_json()
        } else {
            GrinError::new(code, e.message.clone()).to_json()
        };
        // serde_json escapes control characters, so this only fails if the
        // fallback below is somehow wrong.
        let error = CString::new(json)
            .or_else(|_| {
                CString::new(format!(
                    "{{\"code\":{},\"message\":\"error text contained NUL\",\"details\":null}}",
                    GrinErrorCode::Internal as i32
                ))
            })
            .map(CString::into_raw)
            .unwrap_or(std::ptr::null_mut());
        GrinFfiResult {
            code: code as i32,
            data: std::ptr::null_mut(),
            error,
        }
    }
}

/// Serializes a value into the JSON payload of a successful result.
pub(crate) fn to_json<T: Serialize>(value: &T) -> FfiOutcome {
    Ok(Some(serde_json::to_string(value)?))
}

/// `io::Write` sink that only counts bytes.
struct ByteCounter(usize);

impl std::io::Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serializes a value that holds secret text (a mnemonic) so the secret is
/// never left in freed memory. A first pass counts the bytes; the real
/// buffer then gets a capacity of exactly that length plus one, the room
/// `GrinFfiResult::ok` needs for the NUL. With that capacity
/// `CString::new` appends the NUL in place and its `into_boxed_slice` has
/// nothing to shrink, so the buffer is never reallocated (a shrinking
/// realloc can move the block and free the old one unzeroed). There is no
/// intermediate copy; the buffer is zeroed on every error path, and
/// `grin_ffi_result_free` zeroes the final string before freeing it.
pub(crate) fn to_secret_json<T: Serialize>(value: &T) -> FfiOutcome {
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value)?;
    let len = counter.0;
    let mut buf: Vec<u8> = Vec::with_capacity(len + 1);
    if let Err(e) = serde_json::to_writer(&mut buf, value) {
        buf.zeroize();
        return Err(e.into());
    }
    if buf.len() != len {
        // Serialization is deterministic; a different length would mean the
        // buffer grew, so give up rather than return it.
        buf.zeroize();
        return Err(GrinError::internal(
            "secret JSON changed length between passes",
        ));
    }
    match String::from_utf8(buf) {
        Ok(s) => Ok(Some(s)),
        Err(e) => {
            let mut bytes = e.into_bytes();
            bytes.zeroize();
            Err(GrinError::internal("serialized JSON was not UTF-8"))
        }
    }
}

/// Frees a C string allocated by this library, zeroing it first.
///
/// # Safety
/// `ptr` must be NULL or a pointer returned by this library that has not been
/// freed yet.
unsafe fn free_c_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    let mut bytes = CString::from_raw(ptr).into_bytes_with_nul();
    bytes.zeroize();
}

/// Frees `res.data` and `res.error`. NULL members are ignored.
// # Safety
// `res` must be a result returned by this library whose strings have not
// been freed yet. Each one may be freed only once.
#[no_mangle]
pub unsafe extern "C" fn grin_ffi_result_free(res: GrinFfiResult) {
    // Free errors cannot be reported through a void function, so a panic
    // here is logged rather than allowed to cross the C boundary.
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| unsafe {
        free_c_string(res.data);
        free_c_string(res.error);
    })) {
        log_caught_panic("grin_ffi_result_free", payload);
    }
}

/// Frees one string allocated by this library. NULL is ignored.
// # Safety
// `ptr` must be NULL or a string returned by this library that has not
// been freed yet.
#[no_mangle]
pub unsafe extern "C" fn grin_string_free(ptr: *mut c_char) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| unsafe { free_c_string(ptr) })) {
        log_caught_panic("grin_string_free", payload);
    }
}

/// Reads a required string argument. The C caller guarantees `ptr` is NULL or
/// a valid NUL-terminated string that stays alive for the call. The result
/// borrows the pointer variable, so it cannot outlive the FFI call that
/// received it.
pub(crate) fn read_str<'a>(ptr: &'a *const c_char, name: &str) -> Result<&'a str, GrinError> {
    read_opt_str(ptr, name)?
        .ok_or_else(|| GrinError::invalid_argument(format!("{name} must not be NULL")))
}

/// Reads an optional string argument; NULL becomes `None`.
pub(crate) fn read_opt_str<'a>(
    ptr: &'a *const c_char,
    name: &str,
) -> Result<Option<&'a str>, GrinError> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: non-NULL pointers must be valid NUL-terminated strings, which is
    // the documented contract of every entry point taking `const char *`.
    let c = unsafe { CStr::from_ptr(*ptr) };
    c.to_str().map(Some).map_err(|_| {
        GrinError::new(
            GrinErrorCode::InvalidUtf8,
            format!("{name} is not valid UTF-8"),
        )
    })
}

/// Reads an optional string argument and treats an empty string like NULL.
pub(crate) fn read_opt_nonempty<'a>(
    ptr: &'a *const c_char,
    name: &str,
) -> Result<Option<&'a str>, GrinError> {
    Ok(read_opt_str(ptr, name)?
        .map(str::trim)
        .filter(|s| !s.is_empty()))
}

/// Reads a required secret (password) straight into a `ZeroingString`.
pub(crate) fn read_secret(ptr: *const c_char, name: &str) -> Result<ZeroingString, GrinError> {
    read_opt_secret(ptr, name)?
        .ok_or_else(|| GrinError::invalid_argument(format!("{name} must not be NULL")))
}

/// Reads an optional secret (mnemonic) straight into a `ZeroingString`.
pub(crate) fn read_opt_secret(
    ptr: *const c_char,
    name: &str,
) -> Result<Option<ZeroingString>, GrinError> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: see read_opt_str.
    let c = unsafe { CStr::from_ptr(ptr) };
    match c.to_str() {
        Ok(s) => Ok(Some(ZeroingString::from(s))),
        Err(_) => Err(GrinError::new(
            GrinErrorCode::InvalidUtf8,
            format!("{name} is not valid UTF-8"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(res: GrinFfiResult) -> (i32, Option<String>, Option<String>) {
        let read = |p: *mut c_char| {
            if p.is_null() {
                None
            } else {
                Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
            }
        };
        let out = (res.code, read(res.data), read(res.error));
        unsafe { grin_ffi_result_free(res) };
        out
    }

    #[test]
    fn ok_and_error_shapes() {
        let (code, data, err) = take(guard(|| Ok(Some("{\"a\":1}".into()))));
        assert_eq!(code, 0);
        assert_eq!(data.as_deref(), Some("{\"a\":1}"));
        assert!(err.is_none());

        let (code, data, err) = take(guard(|| Ok(None)));
        assert_eq!(code, 0);
        assert!(data.is_none() && err.is_none());

        let (code, data, err) = take(guard(|| {
            Err(GrinError::new(GrinErrorCode::Node, "down")
                .with_details(serde_json::json!({"x": 1})))
        }));
        assert_eq!(code, GrinErrorCode::Node as i32);
        assert!(data.is_none());
        let v: Value = serde_json::from_str(&err.unwrap()).unwrap();
        assert_eq!(v["code"], 9);
        assert_eq!(v["message"], "down");
        assert_eq!(v["details"]["x"], 1);
    }

    #[test]
    fn panic_becomes_internal_panic() {
        let (code, _, err) = take(guard(|| panic!("boom {}", 7)));
        assert_eq!(code, GrinErrorCode::InternalPanic as i32);
        let v: Value = serde_json::from_str(&err.unwrap()).unwrap();
        assert_eq!(v["message"], "boom 7");
        assert!(v["details"].is_null());
    }

    #[test]
    fn interior_nul_becomes_internal() {
        let (code, data, _) = take(guard(|| Ok(Some("a\0b".into()))));
        assert_eq!(code, GrinErrorCode::Internal as i32);
        assert!(data.is_none());
    }

    #[test]
    fn not_enough_funds_details() {
        let e: GrinError = LibwalletError::NotEnoughFunds {
            available: 5,
            available_disp: "5".into(),
            needed: 9,
            needed_disp: "9".into(),
        }
        .into();
        assert_eq!(e.code, GrinErrorCode::InsufficientFunds);
        let d = e.details.unwrap();
        assert_eq!(d["required"], 9);
        assert_eq!(d["available"], 5);
    }

    #[test]
    fn transaction_error_mapping() {
        let e: GrinError = LibwalletError::TransactionCancellationError("no node").into();
        assert_eq!(e.code, GrinErrorCode::Node);
        let e: GrinError = LibwalletError::PaymentProof("Invalid sender signature".into()).into();
        assert_eq!(e.code, GrinErrorCode::Transaction);
        let e: GrinError = LibwalletError::PaymentProofRetrieval("none".into()).into();
        assert_eq!(e.code, GrinErrorCode::NotFound);
        let e: GrinError = LibwalletError::SlateState.into();
        assert_eq!(e.code, GrinErrorCode::Slate);
        let e: GrinError =
            LibwalletError::Backend("DB Not Found Error: Slate id: [1]".into()).into();
        assert_eq!(e.code, GrinErrorCode::NotFound);
        let e: GrinError = LibwalletError::Backend("LMDB error: x".into()).into();
        assert_eq!(e.code, GrinErrorCode::Internal);
        let e: GrinError =
            LibwalletError::GenericError("Transaction amount is too small to include fee".into())
                .into();
        assert_eq!(e.code, GrinErrorCode::Transaction);
    }

    #[test]
    fn lifecycle_errors_are_not_mapped_by_text() {
        // Only a typed decryption error is a wrong password here; Lifecycle
        // text is never inspected, so a data directory with "password" in
        // its path stays a Lifecycle error.
        let e: GrinError = LibwalletError::WalletSeedDecryption.into();
        assert_eq!(e.code, GrinErrorCode::InvalidPassword);
        let e: GrinError = LibwalletError::Lifecycle(
            "Error opening wallet: LMDB error, Data Dir: /home/u/password-store/wallet_data".into(),
        )
        .into();
        assert_eq!(e.code, GrinErrorCode::Lifecycle);
        let e: GrinError = LibwalletError::Lifecycle(
            "New and Old wallet seeds are not equal on password change, not removing backups."
                .into(),
        )
        .into();
        assert_eq!(e.code, GrinErrorCode::Lifecycle);
    }

    #[test]
    fn secret_json_is_never_reallocated() {
        #[derive(Serialize)]
        struct M<'a> {
            mnemonic: &'a str,
        }
        let words = "abandon \"quoted\" word\twith escapes ".repeat(8);
        let s = to_secret_json(&M { mnemonic: &words }).unwrap().unwrap();
        let expected = serde_json::to_string(&M { mnemonic: &words }).unwrap();
        assert_eq!(s, expected);
        // Exactly the room CString::new needs for its NUL, so neither its
        // reserve nor its into_boxed_slice touches the allocation.
        assert_eq!(s.capacity(), s.len() + 1);
        let before = s.as_ptr();
        let c = CString::new(s).unwrap();
        assert_eq!(c.as_ptr().cast::<u8>(), before);
        let res = GrinFfiResult::ok(Some(c.into_string().unwrap()));
        let (code, data, _) = take(res);
        assert_eq!(code, 0);
        assert_eq!(data.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn caught_panic_payloads_are_dropped_safely() {
        struct PanicsOnDrop;
        impl Drop for PanicsOnDrop {
            fn drop(&mut self) {
                panic!("drop of the payload panicked");
            }
        }
        // A payload whose own Drop panics must not escape.
        log_caught_panic("test", Box::new(PanicsOnDrop));
        let (code, _, err) = take(guard(|| std::panic::panic_any(PanicsOnDrop)));
        assert_eq!(code, GrinErrorCode::InternalPanic as i32);
        let v: Value = serde_json::from_str(&err.unwrap()).unwrap();
        assert_eq!(v["message"], "panic with a non-string payload");
    }

    #[test]
    fn read_helpers() {
        assert_eq!(
            read_str(&std::ptr::null(), "x").unwrap_err().code,
            GrinErrorCode::InvalidArgument
        );
        assert!(read_opt_str(&std::ptr::null(), "x").unwrap().is_none());
        let bad = CString::new(vec![0xffu8, 0xfe]).unwrap();
        assert_eq!(
            read_str(&bad.as_ptr(), "x").unwrap_err().code,
            GrinErrorCode::InvalidUtf8
        );
        let good = CString::new("hi").unwrap();
        assert_eq!(read_str(&good.as_ptr(), "x").unwrap(), "hi");
        let empty = CString::new("  ").unwrap();
        assert!(read_opt_nonempty(&empty.as_ptr(), "x").unwrap().is_none());
    }
}
