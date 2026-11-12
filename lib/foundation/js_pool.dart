import 'package:venera/foundation/js_engine.dart';

/// Compatibility entry point; the bounded compute pool is owned by Rust.
class JSPool {
  static final JSPool _instance = JSPool._();
  JSPool._();
  factory JSPool() => _instance;
  Future<void> init() => JsEngine().ensureInit();
  Future<dynamic> execute(String source, List<dynamic> arguments) async {
    await init();
    return JsEngine().compute(source, arguments);
  }
}
