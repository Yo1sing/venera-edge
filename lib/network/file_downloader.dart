import 'package:venera/network/proxy.dart';
import 'package:venera/src/rust/api/download.dart' as backend;
import 'package:venera/src/rust/api/http.dart' as http;

/// Presents Rust download progress to the existing Flutter task widgets.
class FileDownloader {
  final String url;
  final String savePath;
  final int maxConcurrent;
  FileDownloader(this.url, this.savePath, {this.maxConcurrent = 4});
  backend.FileDownload? _task;
  bool _stopped = false;
  bool _started = false;

  Stream<DownloadingStatus> start() async* {
    if (_started) throw StateError('Downloader already started');
    _started = true;
    final proxy = await getProxy();
    if (_stopped) return;
    final task = await backend.FileDownload.start(
      url: url,
      path: savePath,
      concurrency: maxConcurrent,
      options: http.HttpOptions(
        proxy: proxy,
        dnsOverrides: const {},
        tlsSni: true,
        verifyCertificates: true,
        connectTimeoutMs: 15000,
        receiveTimeoutMs: 30000,
        maxRedirects: 10,
      ),
    );
    _task = task;
    try {
      if (_stopped) return;
      while (true) {
        final status = await task.nextStatus();
        if (status == null || _stopped) break;
        yield DownloadingStatus(
          status.downloadedBytes,
          status.totalBytes,
          status.bytesPerSecond,
          status.finished,
        );
      }
    } finally {
      await task.stop();
      _task = null;
      task.dispose();
    }
  }

  Future<void> stop() async {
    _stopped = true;
    await _task?.stop();
  }
}

class DownloadingStatus {
  /// The current downloaded bytes
  final int downloadedBytes;

  /// The total bytes of the file
  final int totalBytes;

  /// Whether the download is finished
  final bool isFinished;

  /// The download speed in bytes per second
  final int bytesPerSecond;

  const DownloadingStatus(
    this.downloadedBytes,
    this.totalBytes,
    this.bytesPerSecond, [
    this.isFinished = false,
  ]);

  @override
  String toString() {
    return "Downloaded: $downloadedBytes/$totalBytes ${isFinished ? "Finished" : ""}";
  }
}
