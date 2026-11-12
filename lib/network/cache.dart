import 'package:venera/src/rust/api/http.dart' as backend;

/// UI-facing cache invalidation; storage and request policy live in Rust.
class NetworkCacheManager {
  void clear() => backend.clearHttpCache();
}
