import 'dart:async';
import 'dart:convert';

import 'package:dio/dio.dart';
import 'dart:typed_data';
import 'package:venera/src/rust/api/http.dart' as backend;
import 'package:venera/foundation/appdata.dart';
import 'package:venera/foundation/log.dart';
import 'package:venera/network/cache.dart';
import 'package:venera/network/proxy.dart';

import '../foundation/app.dart';
import 'cloudflare.dart';
import 'cookie_jar.dart';

export 'package:dio/dio.dart';

class MyLogInterceptor implements Interceptor {
  @override
  void onError(DioException err, ErrorInterceptorHandler handler) {
    Log.error(
      "Network",
      "${err.requestOptions.method} ${err.requestOptions.path}\n$err\n${err.response?.data.toString()}",
    );
    switch (err.type) {
      case DioExceptionType.badResponse:
        var statusCode = err.response?.statusCode;
        if (statusCode != null) {
          err = err.copyWith(
            message:
                "Invalid Status Code: $statusCode. "
                "${_getStatusCodeInfo(statusCode)}",
          );
        }
      case DioExceptionType.connectionTimeout:
        err = err.copyWith(message: "Connection Timeout");
      case DioExceptionType.receiveTimeout:
        err = err.copyWith(
          message:
              "Receive Timeout: "
              "This indicates that the server is too busy to respond",
        );
      case DioExceptionType.unknown:
        if (err.toString().contains("Connection terminated during handshake")) {
          err = err.copyWith(
            message:
                "Connection terminated during handshake: "
                "This may be caused by the firewall blocking the connection "
                "or your requests are too frequent.",
          );
        } else if (err.toString().contains("Connection reset by peer")) {
          err = err.copyWith(
            message:
                "Connection reset by peer: "
                "The error is unrelated to app, please check your network.",
          );
        }
      default:
        {}
    }
    handler.next(err);
  }

  static const errorMessages = <int, String>{
    400: "The Request is invalid.",
    401: "The Request is unauthorized.",
    403: "No permission to access the resource. Check your account or network.",
    404: "Not found.",
    429: "Too many requests. Please try again later.",
  };

  String _getStatusCodeInfo(int? statusCode) {
    if (statusCode != null && statusCode >= 500) {
      return "This is server-side error, please try again later. "
          "Do not report this issue.";
    } else {
      return errorMessages[statusCode] ?? "";
    }
  }

  @override
  void onResponse(
    Response<dynamic> response,
    ResponseInterceptorHandler handler,
  ) {
    var headers = response.headers.map.map(
      (key, value) => MapEntry(
        key.toLowerCase(),
        value.length == 1 ? value.first : value.toString(),
      ),
    );
    headers.remove("cookie");
    String content;
    if (response.data is List<int>) {
      try {
        content = utf8.decode(response.data, allowMalformed: false);
      } catch (e) {
        content = "<Bytes>\nlength:${response.data.length}";
      }
    } else {
      content = response.data.toString();
    }
    Log.addLog(
      (response.statusCode != null && response.statusCode! < 400)
          ? LogLevel.info
          : LogLevel.error,
      "Network",
      "Response ${response.realUri.toString()} ${response.statusCode}\n"
          "headers:\n$headers\n$content",
    );
    handler.next(response);
  }

  @override
  void onRequest(RequestOptions options, RequestInterceptorHandler handler) {
    const String headerMask = "********";
    const String dataMask = "****** DATA_PROTECTED ******";
    Log.info(
      "Network",
      "${options.method} ${options.uri}\n"
          "headers:\n${options.extra.containsKey("maskHeadersInLog") ? options.headers.map((key, value) => MapEntry(key, options.extra["maskHeadersInLog"].contains(key) ? headerMask : value)) : options.headers}\n"
          "data:\n${options.extra["maskDataInLog"] == true ? dataMask : options.data}",
    );
    options.connectTimeout = const Duration(seconds: 15);
    options.receiveTimeout = const Duration(seconds: 15);
    options.sendTimeout = const Duration(seconds: 15);
    handler.next(options);
  }
}

class AppDio with DioMixin {
  AppDio([BaseOptions? options]) {
    this.options = options ?? BaseOptions();
    httpClientAdapter = RustHttpAdapter();
    if (App.isInitialized) {
      interceptors.add(CookieManagerSql(SingleInstanceCookieJar.instance!));
      interceptors.add(NetworkCacheManager());
      interceptors.add(CloudflareInterceptor());
      interceptors.add(MyLogInterceptor());
    }
  }

  static final Map<String, bool> _requests = {};

  @override
  Future<Response<T>> request<T>(
    String path, {
    Object? data,
    Map<String, dynamic>? queryParameters,
    CancelToken? cancelToken,
    Options? options,
    ProgressCallback? onSendProgress,
    ProgressCallback? onReceiveProgress,
  }) async {
    if (options?.headers?['prevent-parallel'] == 'true') {
      while (_requests.containsKey(path)) {
        await Future.delayed(const Duration(milliseconds: 20));
      }
      _requests[path] = true;
      options!.headers!.remove('prevent-parallel');
    }
    try {
      return await super.request<T>(
        path,
        data: data,
        queryParameters: queryParameters,
        cancelToken: cancelToken,
        options: options,
        onSendProgress: onSendProgress,
        onReceiveProgress: onReceiveProgress,
      );
    } finally {
      if (_requests.containsKey(path)) {
        _requests.remove(path);
      }
    }
  }
}

/// Adapts Dio's UI/source-facing API to the Rust HTTP transport.
class RustHttpAdapter implements HttpClientAdapter {
  final _requests = <backend.HttpCancellation>{};
  bool _closed = false;

  @override
  void close({bool force = false}) {
    _closed = true;
    if (force) {
      for (final request in _requests) {
        request.cancel();
      }
    }
  }

  @override
  Future<ResponseBody> fetch(
    RequestOptions options,
    Stream<Uint8List>? requestStream,
    Future<void>? cancelFuture,
  ) async {
    if (_closed) throw StateError('HTTP adapter is closed');
    final cancellation = backend.HttpCancellation();
    _requests.add(cancellation);
    bool finished = false;
    bool cancelled = false;
    backend.HttpResponse? response;
    final upload = backend.HttpUpload();
    final input = requestStream == null ? null : StreamIterator(requestStream);
    Future<void> producer = Future.value();

    Future<void> release() async {
      if (finished) return;
      finished = true;
      _requests.remove(cancellation);
      cancellation.cancel();
      upload.abort();
      await input?.cancel();
      await response?.close();
      await producer;
      response?.dispose();
      upload.dispose();
      cancellation.dispose();
    }

    unawaited(
      cancelFuture?.then((_) {
        if (!finished) {
          cancelled = true;
          cancellation.cancel();
        }
      }),
    );
    try {
      if (input != null) {
        producer = () async {
          try {
            while (await input.moveNext()) {
              if (finished || cancelled) break;
              await upload.write(bytes: input.current);
            }
          } catch (error) {
            if (!finished) await upload.fail(message: error.toString());
          } finally {
            upload.finish();
          }
        }();
      }
      final headers = options.headers.map(
        (key, value) => MapEntry(key, value.toString().trim()),
      );
      if (!headers.keys.any((key) => key.toLowerCase() == 'user-agent')) {
        headers['User-Agent'] = 'venera-edge/v${App.version}';
      }
      final overrides = <String, String>{};
      final config = appdata.settings['dnsOverrides'];
      if (appdata.settings['enableDnsOverrides'] == true && config is Map) {
        for (final entry in config.entries) {
          if (entry.key is String && entry.value is String) {
            overrides[entry.key] = entry.value;
          }
        }
      }
      response = await backend.httpRequest(
        url: options.uri.toString(),
        method: options.method,
        headers: headers.entries
            .map((e) => backend.HttpHeader(name: e.key, value: e.value))
            .toList(),
        upload: upload,
        hasUpload: requestStream != null,
        options: backend.HttpOptions(
          proxy: await getProxy(),
          dnsOverrides: overrides,
          tlsSni: appdata.settings['sni'] != false,
          verifyCertificates: appdata.settings['ignoreBadCertificate'] != true,
          connectTimeoutMs: options.connectTimeout?.inMilliseconds ?? 15000,
          receiveTimeoutMs: options.receiveTimeout?.inMilliseconds ?? 15000,
          maxRedirects: options.followRedirects ? options.maxRedirects : 0,
        ),
        cancellation: cancellation,
      );
      final result = response;
      final responseHeaders = <String, List<String>>{};
      for (final header in result.headers()) {
        (responseHeaders[header.name.toLowerCase()] ??= []).add(header.value);
      }
      Stream<Uint8List> bytes() async* {
        try {
          while (true) {
            final chunk = await result.nextChunk(cancellation: cancellation);
            if (chunk == null) break;
            yield Uint8List.fromList(chunk);
          }
        } finally {
          await release();
        }
      }

      final status = result.status();
      return ResponseBody(
        bytes(),
        status,
        headers: responseHeaders,
        statusMessage: _getStatusMessage(status),
        onClose: () {
          if (!finished) cancellation.cancel();
          unawaited(release());
        },
      );
    } catch (error, stack) {
      await release();
      if (cancelled) {
        throw DioException.requestCancelled(
          requestOptions: options,
          reason: 'Request cancelled',
          stackTrace: stack,
        );
      }
      throw DioException(
        requestOptions: options,
        error: error,
        stackTrace: stack,
      );
    }
  }

  static String _getStatusMessage(int statusCode) {
    return switch (statusCode) {
      200 => "OK",
      201 => "Created",
      202 => "Accepted",
      204 => "No Content",
      206 => "Partial Content",
      301 => "Moved Permanently",
      302 => "Found",
      400 => "Invalid Status Code 400: The Request is invalid.",
      401 => "Invalid Status Code 401: The Request is unauthorized.",
      403 =>
        "Invalid Status Code 403: No permission to access the resource. Check your account or network.",
      404 => "Invalid Status Code 404: Not found.",
      429 =>
        "Invalid Status Code 429: Too many requests. Please try again later.",
      _ => "Invalid Status Code $statusCode",
    };
  }
}
