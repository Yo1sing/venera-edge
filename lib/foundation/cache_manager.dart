import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:venera/src/rust/api/cache.dart';
import 'app.dart';
import 'log.dart';

/// UI adapter. Rust owns cache files, expiry, eviction and the SQLite index.
class CacheManager {
  static String get cachePath => '${App.cachePath}/cache';
  static CacheManager? instance;
  factory CacheManager() => instance ??= CacheManager._create();

  late final Future<CacheStore> _store;
  int _currentSize = 0;
  int get currentSize => _currentSize;

  CacheManager._create() {
    _store = CacheStore.open(dataPath: App.dataPath, cachePath: App.cachePath);
    _runAndLog(_refreshSize());
  }

  void _runAndLog(Future<void> operation) {
    unawaited(
      operation.catchError((Object error, StackTrace stack) {
        Log.error('Cache', error, stack);
      }),
    );
  }

  Future<void> _refreshSize() async {
    _currentSize = await (await _store).size();
  }

  void setLimitSize(int size) {
    _runAndLog(() async {
      await (await _store).setLimit(megabytes: size);
      await _refreshSize();
    }());
  }

  Future<void> writeCache(
    String key,
    List<int> data, [
    int duration = 7 * 24 * 60 * 60 * 1000,
  ]) async {
    await (await _store).write(
      key: key,
      data: data is Uint8List ? data : Uint8List.fromList(data),
      durationMs: duration,
    );
    await _refreshSize();
  }

  Future<File?> findCache(String key) async {
    final path = await (await _store).find(key: key);
    await _refreshSize();
    return path == null ? null : File(path);
  }

  void checkCacheIfRequired() => _runAndLog(checkCache());

  Future<void> checkCache() async {
    await (await _store).prune();
    await _refreshSize();
  }

  Future<void> delete(String key) async {
    await (await _store).delete(key: key);
    await _refreshSize();
  }

  Future<void> clear() async {
    await (await _store).clear();
    await _refreshSize();
  }
}
