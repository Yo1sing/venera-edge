import 'dart:async';
import 'dart:io';
import 'dart:convert';
import 'package:venera/network/file_downloader.dart';
import 'package:venera/src/rust/api/favorites.dart' as favorites;
import 'package:venera/src/rust/api/tags.dart' as tags;
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:venera/foundation/appdata.dart';
import 'package:venera/network/app_dio.dart';
import 'package:venera/src/rust/frb_generated.dart';

void main() {
  final libraryPath = Platform.environment['VENERA_BACKEND_LIBRARY'];
  if (libraryPath == null) {
    test(
      'native backend integration',
      () {},
      skip: 'Build Rust and set VENERA_BACKEND_LIBRARY to run native tests',
    );
    return;
  }
  setUpAll(() async {
    await RustLib.init(externalLibrary: ExternalLibrary.open(libraryPath));
    appdata.settings['proxy'] = 'direct';
  });

  tearDownAll(() {
    RustLib.dispose();
  });

  test('Dio adapter streams binary response through Rust', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final subscription = server.listen((request) async {
      request.response.headers.add('set-cookie', 'a=1');
      request.response.headers.add('set-cookie', 'b=2');
      request.response.add([0, 127, 255]);
      await request.response.close();
    });
    final dio = AppDio();
    try {
      final result = await dio.get<List<int>>(
        'http://127.0.0.1:${server.port}/',
        options: Options(responseType: ResponseType.bytes),
      );
      expect(result.statusCode, 200);
      expect(result.data, [0, 127, 255]);
      expect(result.headers['set-cookie'], ['a=1', 'b=2']);
    } finally {
      dio.close(force: true);
      await subscription.cancel();
      await server.close(force: true);
    }
  });

  test('Dio cancellation reaches a pending Rust HTTP request', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final arrived = Completer<void>();
    final subscription = server.listen((request) {
      arrived.complete();
    });
    final dio = AppDio();
    final token = CancelToken();
    try {
      final future = dio.get<void>(
        'http://127.0.0.1:${server.port}/',
        cancelToken: token,
      );
      final expectation = expectLater(
        future,
        throwsA(
          isA<DioException>().having(
            (e) => e.type,
            'type',
            DioExceptionType.cancel,
          ),
        ),
      );
      await arrived.future.timeout(const Duration(seconds: 5));
      token.cancel();
      await expectation.timeout(const Duration(seconds: 5));
    } finally {
      dio.close(force: true);
      await subscription.cancel();
      await server.close(force: true);
    }
  });

  test('Dio streams uploads larger than the Rust channel capacity', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final subscription = server.listen((request) async {
      final count = await request.fold<int>(
        0,
        (total, chunk) => total + chunk.length,
      );
      request.response.write(count.toString());
      await request.response.close();
    });
    final dio = AppDio();
    try {
      final chunks = List.generate(32, (_) => List<int>.filled(32768, 42));
      final result = await dio.put<String>(
        'http://127.0.0.1:${server.port}/',
        data: Stream<List<int>>.fromIterable(chunks),
        options: Options(
          responseType: ResponseType.plain,
          headers: {'content-length': 32 * 32768},
        ),
      );
      expect(result.data, '${32 * 32768}');
    } finally {
      dio.close(force: true);
      await subscription.cancel();
      await server.close(force: true);
    }
  });
  test(
    'favorites bridge round-trips ordered folders and update metadata',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'venera-favorites-',
      );
      final store = await favorites.FavoriteStore.open(
        path: '${directory.path}/favorites.db',
        traditional: false,
      );
      try {
        store.create(name: 'quoted"folder', renameInvalid: false);
        expect(
          store.add(
            folder: 'quoted"folder',
            comicJson: jsonEncode({
              'id': 'id',
              'name': 'Title',
              'author': 'Author',
              'type': 42,
              'tags': ['tag'],
              'coverPath': 'cover',
            }),
            atEnd: true,
            traditional: false,
          ),
          isTrue,
        );
        store.updateCheck(
          folder: 'quoted"folder',
          id: 'id',
          comicType: 42,
          updateTime: 'today',
        );
        final comic = jsonDecode(
          store.get_(folder: 'quoted"folder', id: 'id', comicType: 42)!,
        );
        expect(comic['has_new_update'], isTrue);
        expect(jsonDecode(store.summary())['total'], 1);
        expect(
          tags.tagSuggestions(text: 'a', traditional: false, limit: 3),
          hasLength(3),
        );
      } finally {
        store.close();
        store.dispose();
        await directory.delete(recursive: true);
      }
    },
  );

  test('file downloader streams a server that ignores Range', () async {
    final directory = await Directory.systemTemp.createTemp('venera-download-');
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final content = Uint8List.fromList(
      List.generate(1024 * 1024, (i) => i % 251),
    );
    final subscription = server.listen((request) async {
      request.response.add(content);
      await request.response.close();
    });
    final file = File('${directory.path}/download');
    final downloader = FileDownloader(
      'http://127.0.0.1:${server.port}/',
      file.path,
    );
    try {
      final progress = await downloader.start().toList();
      expect(progress.last.isFinished, isTrue);
      expect(progress.last.downloadedBytes, content.length);
      expect(await file.readAsBytes(), content);
    } finally {
      await downloader.stop();
      await subscription.cancel();
      await server.close(force: true);
      await directory.delete(recursive: true);
    }
  });

  test(
    'file downloader resumes validated legacy offsets and removes state',
    () async {
      final directory = await Directory.systemTemp.createTemp('venera-resume-');
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final content = Uint8List.fromList(
        List.generate(1024 * 1024, (i) => i % 251),
      );
      final ranges = <String>[];
      final subscription = server.listen((request) async {
        final range = request.headers.value('range')!;
        ranges.add(range);
        final ends = range.substring(6).split('-').map(int.parse).toList();
        request.response.statusCode = 206;
        request.response.headers.set('etag', '"version-1"');
        request.response.headers.set(
          'content-range',
          'bytes ${ends[0]}-${ends[1]}/${content.length}',
        );
        request.response.add(content.sublist(ends[0], ends[1] + 1));
        await request.response.close();
      });
      final file = File('${directory.path}/download');
      final url = 'http://127.0.0.1:${server.port}/';
      final existing = Uint8List(content.length)..setRange(0, 12345, content);
      await file.writeAsBytes(existing);
      await File(
        '${file.path}.download',
      ).writeAsString('0-${content.length}-12345');
      await File('${file.path}.download.meta').writeAsString(
        jsonEncode({
          'url': url,
          'validator': '"version-1"',
          'size': content.length,
        }),
      );
      final downloader = FileDownloader(url, file.path);
      try {
        final progress = await downloader.start().toList();
        expect(progress.last.isFinished, isTrue);
        expect(ranges, ['bytes=0-0', 'bytes=12345-${content.length - 1}']);
        expect(await file.readAsBytes(), content);
        expect(await File('${file.path}.download').exists(), isFalse);
        expect(await File('${file.path}.download.meta').exists(), isFalse);
      } finally {
        await downloader.stop();
        await subscription.cancel();
        await server.close(force: true);
        await directory.delete(recursive: true);
      }
    },
  );

  test(
    'file downloader rejects a mismatched range without claiming success',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'venera-bad-range-',
      );
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final subscription = server.listen((request) async {
        request.response.statusCode = 206;
        request.response.headers.set('content-range', 'bytes 0-0/1024');
        request.response.add([1]);
        await request.response.close();
      });
      final downloader = FileDownloader(
        'http://127.0.0.1:${server.port}/',
        '${directory.path}/file',
      );
      try {
        await expectLater(downloader.start().toList(), throwsA(anything));
      } finally {
        await downloader.stop();
        await subscription.cancel();
        await server.close(force: true);
        await directory.delete(recursive: true);
      }
    },
  );

  test('file downloader cancels an unresponsive server', () async {
    final directory = await Directory.systemTemp.createTemp('venera-cancel-');
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final arrived = Completer<void>();
    final subscription = server.listen((request) {
      arrived.complete();
    });
    final downloader = FileDownloader(
      'http://127.0.0.1:${server.port}/',
      '${directory.path}/file',
    );
    try {
      final progress = downloader.start().toList();
      await arrived.future.timeout(const Duration(seconds: 5));
      await downloader.stop().timeout(const Duration(seconds: 5));
      expect((await progress).any((v) => v.isFinished), isFalse);
    } finally {
      await downloader.stop();
      await subscription.cancel();
      await server.close(force: true);
      await directory.delete(recursive: true);
    }
  });
}
