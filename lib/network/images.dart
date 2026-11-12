import 'dart:async';
import 'dart:typed_data';
import 'package:venera/foundation/app.dart';
import 'package:venera/foundation/js_engine.dart';
import 'package:venera/src/rust/api/media.dart' as backend;

/// Flutter owns subscriptions; source hooks, transport, retries and caching run in Rust.
abstract class ImageDownloader {
  static final _loadingImages = <String, _SharedImageStream>{};

  static Stream<ImageDownloadProgress> loadThumbnail(
    String url,
    String? sourceKey, [
    String? cid,
  ]) => _transfer(url, sourceKey, cid ?? '', '', true);

  static void cancelAllLoadingImages() {
    for (final stream in _loadingImages.values.toList()) {
      stream.cancel();
    }
    _loadingImages.clear();
  }

  static Stream<ImageDownloadProgress> loadComicImage(
    String imageKey,
    String? sourceKey,
    String cid,
    String eid,
  ) {
    final key = '$imageKey@$sourceKey@$cid@$eid';
    final shared = _loadingImages.putIfAbsent(
      key,
      () => _SharedImageStream(
        _transfer(imageKey, sourceKey, cid, eid, false),
        () => _loadingImages.remove(key),
      ),
    );
    return shared.subscribe();
  }

  static Stream<ImageDownloadProgress> loadComicImageUnwrapped(
    String imageKey,
    String? sourceKey,
    String cid,
    String eid,
  ) => _transfer(imageKey, sourceKey, cid, eid, false);

  static Stream<ImageDownloadProgress> _transfer(
    String image,
    String? source,
    String comic,
    String chapter,
    bool thumbnail,
  ) {
    backend.ImageTransfer? transfer;
    var cancelled = false;
    late StreamController<ImageDownloadProgress> controller;
    controller = StreamController(
      onListen: () async {
        try {
          transfer = await backend.ImageTransfer.start(
            runtime: JsEngine().runtime,
            dataPath: App.dataPath,
            cachePath: App.cachePath,
            image: image,
            source: source,
            comic: comic,
            chapter: chapter,
            thumbnail: thumbnail,
          );
          if (cancelled) {
            transfer!.cancel();
            return;
          }
          while (!cancelled) {
            final event = await transfer!.next();
            if (event == null || cancelled) break;
            if (event.error != null) throw StateError(event.error!);
            controller.add(
              ImageDownloadProgress(
                currentBytes: event.current,
                totalBytes: event.total,
                imageBytes: event.bytes,
              ),
            );
            if (event.bytes != null) break;
          }
        } catch (error, stack) {
          if (!cancelled) controller.addError(error, stack);
        } finally {
          transfer?.cancel();
          transfer?.dispose();
          transfer = null;
          unawaited(controller.close());
        }
      },
      onCancel: () {
        cancelled = true;
        transfer?.cancel();
      },
    );
    return controller.stream;
  }
}

class _SharedImageStream {
  final Stream<ImageDownloadProgress> input;
  final void Function() onClosed;
  final listeners = <StreamController<ImageDownloadProgress>>{};
  StreamSubscription<ImageDownloadProgress>? subscription;
  _SharedImageStream(this.input, this.onClosed);
  Stream<ImageDownloadProgress> subscribe() {
    late StreamController<ImageDownloadProgress> controller;
    controller = StreamController(
      onListen: () {
        listeners.add(controller);
        subscription ??= input.listen(
          (event) {
            for (final listener in listeners.toList()) {
              listener.add(event);
            }
          },
          onError: (Object error, StackTrace stack) {
            for (final listener in listeners.toList()) {
              listener.addError(error, stack);
            }
          },
          onDone: cancel,
        );
      },
      onCancel: () {
        listeners.remove(controller);
        if (listeners.isEmpty) cancel();
      },
    );
    return controller.stream;
  }

  void cancel() {
    final active = subscription;
    subscription = null;
    unawaited(active?.cancel());
    final controllers = listeners.toList();
    listeners.clear();
    for (final controller in controllers) {
      unawaited(controller.close());
    }
    onClosed();
  }
}

class ImageDownloadProgress {
  final int currentBytes;
  final int? totalBytes;
  final Uint8List? imageBytes;
  const ImageDownloadProgress({
    required this.currentBytes,
    required this.totalBytes,
    this.imageBytes,
  });
}
