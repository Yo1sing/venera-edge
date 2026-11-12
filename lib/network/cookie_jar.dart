import 'dart:io';

import 'package:dio/dio.dart';
import 'package:path_provider/path_provider.dart';
import 'package:venera/src/rust/api/cookies.dart';

/// Compatibility adapter for platform Cookie values and the source JavaScript API.
/// Persistence, expiry and request matching are owned by Rust.
class CookieJarSql {
  final String path;
  CookieStore? _store;

  CookieJarSql(this.path) {
    init();
  }

  void init() {
    _store?.close();
    _store = CookieStore.open(path: path);
  }

  void saveFromResponse(Uri uri, List<Cookie> cookies) {
    _store!.save(
      url: uri.toString(),
      cookies: cookies
          .map(
            (cookie) => StoredCookie(
              name: cookie.name,
              value: cookie.value,
              domain: cookie.domain,
              path: cookie.path,
              expiresMs: cookie.expires?.millisecondsSinceEpoch,
              secure: cookie.secure,
              httpOnly: cookie.httpOnly,
              maxAge: cookie.maxAge,
            ),
          )
          .toList(),
    );
  }

  List<Cookie> loadForRequest(Uri uri) => _store!
      .load(url: uri.toString())
      .map(
        (cookie) => Cookie(cookie.name, cookie.value)
          ..domain = cookie.domain
          ..path = cookie.path
          ..expires = cookie.expiresMs == null
              ? null
              : DateTime.fromMillisecondsSinceEpoch(cookie.expiresMs!)
          ..secure = cookie.secure
          ..httpOnly = cookie.httpOnly,
      )
      .toList();

  void saveFromResponseCookieHeader(Uri uri, List<String> headers) =>
      _store!.saveHeaders(url: uri.toString(), headers: headers);

  String loadForRequestCookieHeader(Uri uri) =>
      _store!.header(url: uri.toString());

  void delete(Uri uri, String name) =>
      _store!.delete(url: uri.toString(), name: name);

  void deleteUri(Uri uri) => _store!.delete(url: uri.toString());

  void deleteAll() => _store!.clear();

  void dispose() {
    _store?.close();
    _store = null;
  }
}

class SingleInstanceCookieJar extends CookieJarSql {
  factory SingleInstanceCookieJar(String path) =>
      instance ??= SingleInstanceCookieJar._create(path);

  SingleInstanceCookieJar._create(super.path);

  static SingleInstanceCookieJar? instance;

  static Future<SingleInstanceCookieJar> createInstance() async {
    if (instance != null) {
      return instance!;
    }
    var dataPath = (await getApplicationSupportDirectory()).path;
    instance = SingleInstanceCookieJar("$dataPath/cookie.db");
    return instance!;
  }
}

class CookieManagerSql extends Interceptor {
  final CookieJarSql cookieJar;

  CookieManagerSql(this.cookieJar);

  @override
  void onRequest(RequestOptions options, RequestInterceptorHandler handler) {
    var cookies = cookieJar.loadForRequestCookieHeader(options.uri);
    if (cookies.isNotEmpty) {
      if (options.headers["cookie"] != null) {
        cookies = "${options.headers["cookie"]}; $cookies";
      }
      options.headers["cookie"] = cookies;
    }
    handler.next(options);
  }

  @override
  void onResponse(Response response, ResponseInterceptorHandler handler) {
    cookieJar.saveFromResponseCookieHeader(
      response.requestOptions.uri,
      response.headers["set-cookie"] ?? [],
    );
    handler.next(response);
  }

  @override
  void onError(DioException err, ErrorInterceptorHandler handler) {
    handler.next(err);
  }
}
