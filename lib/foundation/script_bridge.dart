import 'dart:convert';
import 'dart:typed_data';
import 'package:venera/src/rust/api/script.dart' as backend;

/// Transport and lifetime adapter for Rust-owned JavaScript values.
class ScriptSession {
  final backend.ScriptRuntime runtime;
  bool closed = false;
  ScriptSession(this.runtime);

  dynamic decodeJson(String json) => decode(jsonDecode(json));
  dynamic decode(dynamic wire) {
    switch (wire[0]) {
      case 'null':
        return null;
      case 'bool':
      case 'string':
        return wire[1];
      case 'number':
        return wire[1] is num ? wire[1] : double.parse(wire[1]);
      case 'bigint':
        return BigInt.parse(wire[1]);
      case 'bytes':
        return Uint8List.fromList(List<int>.from(wire[1]));
      case 'array':
        return (wire[1] as List).map(decode).toList();
      case 'object':
        return <String, dynamic>{
          for (final entry in wire[1]) entry[0] as String: decode(entry[1]),
        };
      case 'function':
        return JSInvokable._(this, wire[1]);
      case 'promise':
        return runtime.awaitPromise(id: wire[1]).then(decodeJson);
      default:
        throw StateError('Unknown JavaScript bridge value: ${wire[0]}');
    }
  }

  dynamic evaluate(String source) =>
      decodeJson(runtime.evaluate(source: source));
  void close() {
    if (closed) return;
    closed = true;
    runtime.close();
  }
}

dynamic encodeScriptValue(dynamic value) {
  if (value == null) return ['null'];
  if (value is bool) return ['bool', value];
  if (value is num) {
    return ['number', value.isFinite ? value : value.toString()];
  }
  if (value is BigInt) return ['bigint', value.toString()];
  if (value is String) return ['string', value];
  if (value is Uint8List) return ['bytes', value.toList()];
  if (value is ByteBuffer) return ['bytes', value.asUint8List().toList()];
  if (value is JSInvokable) return ['function', value._handle.id];
  if (value is List) return ['array', value.map(encodeScriptValue).toList()];
  if (value is Map) {
    return [
      'object',
      value.entries
          .map((e) => [e.key.toString(), encodeScriptValue(e.value)])
          .toList(),
    ];
  }
  throw ArgumentError('Unsupported JavaScript argument: ${value.runtimeType}');
}

class _FunctionHandle {
  final ScriptSession session;
  final int id;
  int references = 1;
  _FunctionHandle(this.session, this.id);
  void release() {
    if (references == 0) return;
    references--;
    if (!session.closed) session.runtime.release(id: id);
  }

  void dispose() {
    while (references > 0) {
      release();
    }
  }
}

class JSInvokable {
  final _FunctionHandle _handle;
  static final _finalizer = Finalizer<_FunctionHandle>((handle) {
    try {
      handle.dispose();
    } catch (_) {
      /* Runtime may have already closed. */
    }
  });
  JSInvokable._(ScriptSession session, int id)
    : _handle = _FunctionHandle(session, id) {
    _finalizer.attach(this, _handle, detach: this);
  }
  dynamic call(List<dynamic> arguments) {
    if (_handle.references == 0 || _handle.session.closed) {
      throw StateError('JavaScript function is released');
    }
    return _handle.session.decodeJson(
      _handle.session.runtime.invoke(
        id: _handle.id,
        argumentsJson: jsonEncode(encodeScriptValue(arguments)),
      ),
    );
  }

  dynamic invoke(List<dynamic> arguments) => call(arguments);
  void dup() {
    if (_handle.references == 0 || _handle.session.closed) {
      throw StateError('JavaScript function is released');
    }
    _handle.session.runtime.retain(id: _handle.id);
    _handle.references++;
  }

  void free() => _handle.release();
  void destroy() {
    _handle.dispose();
    _finalizer.detach(this);
  }
}
