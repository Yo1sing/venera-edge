// Run after `cargo build --manifest-path rust/Cargo.toml`:
// dart run test/backend_bridge_smoke.dart <path-to-native-library>
import 'dart:convert';
import 'dart:io';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:venera/src/rust/frb_generated.dart';
import 'package:venera/src/rust/api/cache.dart';
import 'package:venera/src/rust/api/cookies.dart';
import 'package:venera/src/rust/api/history.dart';
import 'package:venera/src/rust/api/text.dart';
import 'package:venera/src/rust/api/local.dart';

void expect(bool condition, String message) {
  if (!condition) throw StateError(message);
}

Future<void> main(List<String> args) async {
  if (args.length != 1) throw ArgumentError('Pass the native library path');
  await RustLib.init(externalLibrary: ExternalLibrary.open(args.single));
  final directory = await Directory.systemTemp.createTemp('venera-frb-test-');
  CacheStore? cache;
  CookieStore? cookies;
  HistoryStore? history;
  LocalStore? local;
  try {
    expect(simplifiedToTraditional(text: '漫画') == '漫畫', 'text codec');
    cache = await CacheStore.open(
      dataPath: directory.path,
      cachePath: directory.path,
    );
    await cache.write(key: 'cover', data: [0, 127, 255], durationMs: 10000);
    final path = await cache.find(key: 'cover');
    expect(
      path != null && (await File(path).readAsBytes()).last == 255,
      'bytes round trip',
    );
    expect(await cache.size() == 3, '64-bit integer codec');

    cookies = CookieStore.open(path: '${directory.path}/cookie.db');
    cookies.saveHeaders(
      url: 'https://example.com/',
      headers: ['session=abc; Path=/; Secure'],
    );
    expect(
      cookies.header(url: 'https://example.com/') == 'session=abc',
      'cookie persistence',
    );
    expect(
      cookies.load(url: 'http://example.com/').isEmpty,
      'secure cookie scope',
    );

    history = await HistoryStore.open(path: '${directory.path}/history.db');
    await history.save(
      entry: const HistoryEntry(
        id: 'comic',
        title: 'Title',
        subtitle: 'Author',
        cover: 'cover',
        timeMs: 1700000000000,
        comicType: 123,
        episode: 2,
        page: 4,
        readEpisodes: ['1', '2'],
        maxPage: 50,
        chapterGroup: 3,
      ),
    );
    final entry = history.find(id: 'comic', comicType: 123);
    expect(
      entry?.chapterGroup == 3 && entry?.timeMs == 1700000000000,
      'history DTO',
    );
    expect(history.find(id: 'missing', comicType: 123) == null, 'optional DTO');
    expect(
      jsonDecode(await history.imageStatistics())['count'] == 0,
      'async statistics',
    );
    local = await LocalStore.open(path: '${directory.path}/local.db');
    local.save(
      comicJson: jsonEncode({
        'id': '1',
        'title': 'Local',
        'subtitle': 'Author',
        'tags': ['tag'],
        'directory': 'folder',
        'chapters': {'z': 'First', 'a': 'Second'},
        'cover': 'cover.jpg',
        'comic_type': 0,
        'downloaded_chapters': ['z'],
        'created_at': 1700000000000,
      }),
    );
    expect(
      local.downloaded(id: '1', comicType: 0, episode: 1),
      'local chapter order',
    );
    expect(
      !local.downloaded(id: '1', comicType: 0, episode: 2),
      'local download state',
    );
    expect(
      sortComicImages(
            paths: ['a/10.jpg', 'a/2.jpg', 'a/.hidden', 'a/cover.jpg'],
          ).join(',') ==
          'file://a/2.jpg,file://a/10.jpg',
      'image filtering and ordering',
    );
    print(
      'PASS: native FRB initialization, bytes, integers, DTOs, options, sync and async calls',
    );
  } finally {
    local?.close();
    local?.dispose();
    cookies?.close();
    cookies?.dispose();
    history?.close();
    history?.dispose();
    // Explicitly dispose the opaque handle before removing its database on Windows.
    cache?.dispose();
    RustLib.dispose();
    await directory.delete(recursive: true);
  }
}
