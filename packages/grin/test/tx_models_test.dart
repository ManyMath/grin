import 'dart:convert';

import 'package:grin/grin.dart';
import 'package:test/test.dart';

const String _alice =
    'tgrin1x7a32gk5qdwx9kdq2tf8hwucvtnjvfxhq2tcn3e3jxcyw0hc7dxs2exm5s';
const String _bob =
    'tgrin1m2mfwpkg9gf8lmwzlqrq7pcguejzvh5rjq8q0qa6fxcknpdnxrfsm4w7yd';
const String _slateId = '2b1c7f2e-9d4a-4b8e-a3c5-0f6d1e2a3b4c';
const String _excess =
    '08a1b2c3d4e5f60718293a4b5c6d7e8f9011223344556677889900aabbccddeeff';

Map<String, Object?> _slatepackJson() => <String, Object?>{
  'slate_id': _slateId,
  'state': 'S1',
  'armored_slatepack': 'BEGINSLATEPACK. abc. ENDSLATEPACK.\n',
  'amount': 50000000000,
  'fee': 23500000,
  'sender': _alice,
  'recipients': <Object?>[_bob],
  'is_encrypted': true,
};

Map<String, Object?> _entryJson() => <String, Object?>{
  'id': 7,
  'slate_id': _slateId,
  'slate_state': 'S3',
  'type': 'TxSent',
  'creation_ts': '2026-03-01T12:30:45.123456789Z',
  'confirmation_ts': '2026-03-01T12:45:00Z',
  'confirmed': true,
  'num_inputs': 1,
  'num_outputs': 1,
  'amount_credited': 59976500000,
  'amount_debited': 110000000000,
  'fee': 23500000,
  'ttl_cutoff_height': null,
  'kernel_excess': _excess,
  'kernel_lookup_min_height': 4,
  'has_stored_tx': true,
  'payment_proof': <String, Object?>{
    'receiver_address': _bob,
    'sender_address': _alice,
    'receiver_signature_present': true,
    'sender_signature_present': true,
  },
  'counterparty_address': _bob,
  'armored_slatepack': 'BEGINSLATEPACK. abc. ENDSLATEPACK.\n',
  'reverted_after_secs': null,
};

Map<String, Object?> _proofJson() => <String, Object?>{
  'amount': '50000000000',
  'excess': _excess,
  'recipient_address': _bob,
  'recipient_sig': 'ab' * 64,
  'sender_address': _alice,
  'sender_sig': 'cd' * 64,
};

void main() {
  group('GrinSlateState and GrinTxType', () {
    test('parse every wire name and fall back to unknown', () {
      for (final state in GrinSlateState.values) {
        expect(GrinSlateState.parse(state.wireName), state);
      }
      expect(GrinSlateState.parse('S1'), GrinSlateState.s1);
      expect(GrinSlateState.parse('I3'), GrinSlateState.i3);
      expect(GrinSlateState.parse('Unknown'), GrinSlateState.unknown);
      expect(GrinSlateState.parse('S9'), GrinSlateState.unknown);

      for (final type in GrinTxType.values) {
        expect(GrinTxType.parse(type.wireName), type);
      }
      expect(GrinTxType.parse('TxSentCancelled'), GrinTxType.txSentCancelled);
      expect(GrinTxType.parse('Something'), GrinTxType.unknown);
      expect(GrinTxType.txSentCancelled.isCancelled, isTrue);
      expect(GrinTxType.txReceivedCancelled.isCancelled, isTrue);
      expect(GrinTxType.txSent.isCancelled, isFalse);
    });
  });

  group('GrinFeeEstimate', () {
    test('fromJson reads every field', () {
      final estimate = GrinFeeEstimate.fromJson({
        'fee': 23500000,
        'amount': 50000000000,
        'net_amount': 50000000000,
        'total': 50023500000,
        'inputs_count': 1,
        'change_outputs': 1,
      });
      expect(estimate.fee, BigInt.from(23500000));
      expect(estimate.amount, BigInt.from(50000000000));
      expect(estimate.netAmount, estimate.amount);
      expect(estimate.total, BigInt.from(50023500000));
      expect(estimate.inputsCount, 1);
      expect(estimate.changeOutputs, 1);
      expect(estimate, estimate);
      expect(estimate.hashCode, estimate.hashCode);
    });

    test('a missing field is a FormatException', () {
      expect(() => GrinFeeEstimate.fromJson({'fee': 1}), throwsFormatException);
    });
  });

  group('GrinSlatepack', () {
    test('fromJson reads the S1 document', () {
      final pack = GrinSlatepack.fromJson(_slatepackJson());
      expect(pack.slateId, _slateId);
      expect(pack.state, GrinSlateState.s1);
      expect(pack.armoredSlatepack, startsWith('BEGINSLATEPACK.'));
      expect(pack.amount, BigInt.from(50000000000));
      expect(pack.fee, BigInt.from(23500000));
      expect(pack.sender, _alice);
      expect(pack.recipients, [_bob]);
      expect(pack.isEncrypted, isTrue);
      expect(GrinSlatepack.fromJson(_slatepackJson()), pack);
      expect(GrinSlatepack.fromJson(_slatepackJson()).hashCode, pack.hashCode);
    });

    test('a different recipient list is not equal', () {
      final other = _slatepackJson()..['recipients'] = <Object?>[];
      expect(GrinSlatepack.fromJson(other), isNot(_slatepackJsonModel()));
    });

    test('a wrong type is a FormatException', () {
      final bad = _slatepackJson()..['recipients'] = 'tgrin1';
      expect(() => GrinSlatepack.fromJson(bad), throwsFormatException);
    });
  });

  group('GrinSlateInfo', () {
    test('fromJson reads an S3 with null fields', () {
      final info = GrinSlateInfo.fromJson({
        'slate_id': _slateId,
        'state': 'S3',
        'amount': 0,
        'fee': 23500000,
        'kernel_excess': _excess,
        'sender': null,
        'recipients': <Object?>[],
        'ttl_cutoff_height': null,
        'num_participants': 2,
        'is_encrypted': false,
        'slate_json': '{"id":"$_slateId"}',
      });
      expect(info.state, GrinSlateState.s3);
      expect(info.amount, BigInt.zero);
      expect(info.kernelExcess, _excess);
      expect(info.sender, isNull);
      expect(info.recipients, isEmpty);
      expect(info.ttlCutoffHeight, isNull);
      expect(info.numParticipants, 2);
      expect(info.isEncrypted, isFalse);
      expect(jsonDecode(info.slateJson), {'id': _slateId});
    });

    test('a pre-V4 state is unknown', () {
      final info = GrinSlateInfo.fromJson({
        'slate_id': _slateId,
        'state': 'Unknown',
        'amount': 1,
        'fee': 0,
        'kernel_excess': null,
        'sender': _alice,
        'recipients': <Object?>[],
        'ttl_cutoff_height': 99,
        'num_participants': 2,
        'is_encrypted': false,
        'slate_json': '{}',
      });
      expect(info.state, GrinSlateState.unknown);
      expect(info.ttlCutoffHeight, 99);
    });
  });

  group('GrinTxLogEntry and GrinTxList', () {
    test('fromJson reads a confirmed send', () {
      final entry = GrinTxLogEntry.fromJson(_entryJson());
      expect(entry.id, 7);
      expect(entry.slateId, _slateId);
      expect(entry.slateState, GrinSlateState.s3);
      expect(entry.type, GrinTxType.txSent);
      expect(
        entry.creationTime,
        DateTime.utc(2026, 3, 1, 12, 30, 45, 123, 456),
      );
      expect(entry.creationTime.isUtc, isTrue);
      expect(entry.confirmationTime, DateTime.utc(2026, 3, 1, 12, 45));
      expect(entry.confirmed, isTrue);
      expect(entry.amountCredited, BigInt.from(59976500000));
      expect(entry.amountDebited, BigInt.from(110000000000));
      expect(entry.netAmount, BigInt.from(-50023500000));
      expect(entry.fee, BigInt.from(23500000));
      expect(entry.ttlCutoffHeight, isNull);
      expect(entry.kernelExcess, _excess);
      expect(entry.kernelLookupMinHeight, 4);
      expect(entry.hasStoredTx, isTrue);
      expect(entry.paymentProof?.receiverAddress, _bob);
      expect(entry.paymentProof?.senderAddress, _alice);
      expect(entry.paymentProof?.receiverSignaturePresent, isTrue);
      expect(entry.paymentProof?.senderSignaturePresent, isTrue);
      expect(entry.counterpartyAddress, _bob);
      expect(entry.armoredSlatepack, contains('SLATEPACK'));
      expect(entry.revertedAfterSecs, isNull);
      expect(GrinTxLogEntry.fromJson(_entryJson()), entry);
      expect(GrinTxLogEntry.fromJson(_entryJson()).hashCode, entry.hashCode);
    });

    test('a coinbase entry has null optional fields', () {
      final json = _entryJson()
        ..['slate_id'] = null
        ..['slate_state'] = null
        ..['type'] = 'ConfirmedCoinbase'
        ..['confirmation_ts'] = null
        ..['fee'] = null
        ..['kernel_excess'] = null
        ..['kernel_lookup_min_height'] = null
        ..['payment_proof'] = null
        ..['counterparty_address'] = null
        ..['armored_slatepack'] = null
        ..['has_stored_tx'] = false;
      final entry = GrinTxLogEntry.fromJson(json);
      expect(entry.slateId, isNull);
      expect(entry.slateState, isNull);
      expect(entry.type, GrinTxType.confirmedCoinbase);
      expect(entry.confirmationTime, isNull);
      expect(entry.fee, isNull);
      expect(entry.paymentProof, isNull);
      expect(entry.counterpartyAddress, isNull);
    });

    test('a bad timestamp is a FormatException', () {
      final json = _entryJson()..['creation_ts'] = 'yesterday';
      expect(() => GrinTxLogEntry.fromJson(json), throwsFormatException);
    });

    test('GrinTxList reads the entries in order', () {
      final second = _entryJson()
        ..['id'] = 8
        ..['slate_id'] = '6f1c3e1a-0d4b-4c39-9b6e-6c1f6f0d2a11';
      final list = GrinTxList.fromJson({
        'refreshed_from_node': true,
        'txs': <Object?>[_entryJson(), second],
      });
      expect(list.refreshedFromNode, isTrue);
      expect(list.transactions.map((e) => e.id), [7, 8]);
      expect(list.bySlateId(_slateId)?.id, 7);
      expect(list.bySlateId('unknown'), isNull);
      expect(
        GrinTxList.fromJson({
          'refreshed_from_node': true,
          'txs': <Object?>[_entryJson(), second],
        }),
        list,
      );
      expect(
        () =>
            GrinTxList.fromJson({'refreshed_from_node': false, 'txs': 'nope'}),
        throwsFormatException,
      );
    });
  });

  group('GrinFinalizeResult and GrinSendResult', () {
    test('finalize result fromJson', () {
      final result = GrinFinalizeResult.fromJson({
        'slate_id': _slateId,
        'state': 'I3',
        'amount': 30000000000,
        'fee': 23500000,
        'kernel_excess': _excess,
        'posted': false,
        'armored_slatepack': 'BEGINSLATEPACK. x. ENDSLATEPACK.\n',
      });
      expect(result.state, GrinSlateState.i3);
      expect(result.amount, BigInt.from(30000000000));
      expect(result.fee, BigInt.from(23500000));
      expect(result.kernelExcess, _excess);
      expect(result.posted, isFalse);
      expect(result.armoredSlatepack, contains('SLATEPACK'));
    });

    test('send result fromJson', () {
      final result = GrinSendResult.fromJson({
        'slate_id': _slateId,
        'state': 'S3',
        'amount': 1000000000,
        'fee': 7000000,
        'kernel_excess': _excess,
        'posted': true,
      });
      expect(result.state, GrinSlateState.s3);
      expect(result.posted, isTrue);
      expect(result.kernelExcess, _excess);
      expect(result.slateId, _slateId);
    });
  });

  group('send and invoice arguments', () {
    test('GrinSendArgs defaults match args_json', () {
      expect(GrinSendArgs(amount: BigInt.from(50000000000)).toJson(), {
        'amount': 50000000000,
        'min_confirms': 10,
        'selection_strategy_is_all': false,
        'amount_includes_fee': false,
        'recipient_address': null,
        'ttl_blocks': null,
        'message': null,
        'late_lock': false,
      });
    });

    test('GrinSendArgs with every option', () {
      final args = GrinSendArgs(
        amount: BigInt.from(5),
        minimumConfirmations: 1,
        selectionStrategyIsAll: true,
        amountIncludesFee: true,
        recipientAddress: _bob,
        ttlBlocks: 60,
        message: 'note',
        lateLock: true,
      );
      expect(args.toJson(), {
        'amount': 5,
        'min_confirms': 1,
        'selection_strategy_is_all': true,
        'amount_includes_fee': true,
        'recipient_address': _bob,
        'ttl_blocks': 60,
        'message': 'note',
        'late_lock': true,
      });
      expect(jsonDecode(jsonEncode(args.toJson())), args.toJson());
    });

    test('GrinHttpSendArgs adds the target and broadcast options', () {
      final args = GrinHttpSendArgs(
        amount: BigInt.from(1000000000),
        recipientUrlOrAddress: 'http://127.0.0.1:3415',
        minimumConfirmations: 3,
        recipientAddress: _bob,
        fluff: true,
        post: false,
      );
      expect(args.toJson(), {
        'amount': 1000000000,
        'min_confirms': 3,
        'selection_strategy_is_all': false,
        'amount_includes_fee': false,
        'recipient_address': _bob,
        'ttl_blocks': null,
        'message': null,
        'late_lock': false,
        'recipient_url_or_address': 'http://127.0.0.1:3415',
        'fluff': true,
        'post': false,
      });
      final defaults = GrinHttpSendArgs(
        amount: BigInt.one,
        recipientUrlOrAddress: _bob,
      ).toJson();
      expect(defaults['fluff'], false);
      expect(defaults['post'], true);
    });

    test('GrinInvoiceArgs and GrinProcessInvoiceArgs', () {
      expect(
        GrinInvoiceArgs(
          amount: BigInt.from(30000000000),
          recipientAddress: _alice,
        ).toJson(),
        {
          'amount': 30000000000,
          'dest_acct_name': null,
          'recipient_address': _alice,
        },
      );
      expect(
        GrinInvoiceArgs(
          amount: BigInt.one,
          destinationAccountName: 'savings',
        ).toJson(),
        {'amount': 1, 'dest_acct_name': 'savings', 'recipient_address': null},
      );
      expect(const GrinProcessInvoiceArgs().toJson(), {
        'min_confirms': 10,
        'selection_strategy_is_all': false,
        'ttl_blocks': null,
      });
      expect(
        const GrinProcessInvoiceArgs(
          minimumConfirmations: 1,
          selectionStrategyIsAll: true,
          ttlBlocks: 20,
        ).toJson(),
        {
          'min_confirms': 1,
          'selection_strategy_is_all': true,
          'ttl_blocks': 20,
        },
      );
    });

    test('an amount outside the native range is an ArgumentError', () {
      final tooBig = BigInt.one << 63;
      for (final bad in [BigInt.from(-1), tooBig, BigInt.one << 64]) {
        expect(() => GrinSendArgs(amount: bad).toJson(), throwsArgumentError);
        expect(
          () => GrinHttpSendArgs(
            amount: bad,
            recipientUrlOrAddress: _bob,
          ).toJson(),
          throwsArgumentError,
        );
        expect(
          () => GrinInvoiceArgs(amount: bad).toJson(),
          throwsArgumentError,
        );
      }
      expect(
        GrinSendArgs(amount: tooBig - BigInt.one).toJson()['amount'],
        9223372036854775807,
      );
    });

    test('equal arguments are equal', () {
      expect(
        GrinSendArgs(amount: BigInt.one),
        GrinSendArgs(amount: BigInt.one),
      );
      expect(
        GrinSendArgs(amount: BigInt.one),
        isNot(GrinSendArgs(amount: BigInt.two)),
      );
      expect(
        GrinSendArgs(amount: BigInt.one).hashCode,
        GrinSendArgs(amount: BigInt.one).hashCode,
      );
    });
  });

  group('GrinPaymentProof', () {
    test('keeps the proof text byte for byte', () {
      final text = jsonEncode(_proofJson());
      final proof = GrinPaymentProof.fromJsonString(text);
      expect(proof.toJsonString(), text);
      expect(proof.amount, BigInt.from(50000000000));
      expect(proof.excess, _excess);
      expect(proof.recipientAddress, _bob);
      expect(proof.senderAddress, _alice);
      expect(proof.recipientSignature, 'ab' * 64);
      expect(proof.senderSignature, 'cd' * 64);
      expect(GrinPaymentProof.fromJsonString(proof.toJsonString()), proof);
    });

    test('accepts the export document and the bare proof', () {
      final bare = GrinPaymentProof.fromJson(_proofJson());
      final wrapped = GrinPaymentProof.fromJson({'proof': _proofJson()});
      expect(wrapped, bare);
      expect(wrapped.toJsonString(), bare.toJsonString());
      expect(bare.toJson(), _proofJson());
    });

    test('toJson returns a copy', () {
      final proof = GrinPaymentProof.fromJson(_proofJson());
      proof.toJson()['amount'] = '1';
      expect(proof.toJson()['amount'], '50000000000');
    });

    test('a changed field makes a different proof', () {
      final proof = GrinPaymentProof.fromJson(_proofJson());
      final tampered = GrinPaymentProof.fromJson({
        ...proof.toJson(),
        'amount': '20',
      });
      expect(tampered, isNot(proof));
      expect(tampered.amount, BigInt.from(20));
    });

    test('a missing field is a FormatException', () {
      final json = _proofJson()..remove('sender_sig');
      expect(() => GrinPaymentProof.fromJson(json), throwsFormatException);
      expect(
        () => GrinPaymentProof.fromJsonString('[1]'),
        throwsFormatException,
      );
    });

    test('verification fromJson', () {
      final result = GrinPaymentProofVerification.fromJson({
        'is_sender': true,
        'is_recipient': false,
      });
      expect(result.isSender, isTrue);
      expect(result.isRecipient, isFalse);
      expect(
        result,
        const GrinPaymentProofVerification(isSender: true, isRecipient: false),
      );
    });
  });
}

GrinSlatepack _slatepackJsonModel() => GrinSlatepack.fromJson(_slatepackJson());
