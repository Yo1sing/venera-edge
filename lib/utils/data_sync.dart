import 'dart:convert';
import 'package:venera/foundation/js_engine.dart';
import 'package:venera/src/rust/api/backup.dart' as backend;
import 'package:flutter/foundation.dart';
import 'package:venera/components/components.dart';
import 'package:venera/components/window_frame.dart';
import 'package:venera/foundation/app.dart';
import 'package:venera/foundation/appdata.dart';
import 'package:venera/foundation/comic_source/comic_source.dart';
import 'package:venera/foundation/favorites.dart';
import 'package:venera/foundation/log.dart';
import 'package:venera/foundation/res.dart';
import 'package:venera/utils/data.dart';
import 'package:venera/utils/translations.dart';

import 'io.dart';

class DataSync with ChangeNotifier {
  DataSync._() {
    if (isEnabled) {
      downloadData();
    }
    LocalFavoritesManager().addListener(onDataChanged);
    ComicSourceManager().addListener(onDataChanged);
    if (App.isDesktop) {
      Future.delayed(const Duration(seconds: 1), () {
        var controller = WindowFrame.of(App.rootContext);
        controller.addCloseListener(_handleWindowClose);
      });
    }
  }

  void onDataChanged() {
    if (isEnabled) {
      uploadData();
    }
  }

  bool _handleWindowClose() {
    if (_isUploading) {
      _showWindowCloseDialog();
      return false;
    }
    return true;
  }

  void _showWindowCloseDialog() async {
    showLoadingDialog(
      App.rootContext,
      cancelButtonText: "Shut Down".tl,
      onCancel: () => exit(0),
      barrierDismissible: false,
      message: "Uploading data...".tl,
    );
    while (_isUploading) {
      await Future.delayed(const Duration(milliseconds: 50));
    }
    exit(0);
  }

  static DataSync? instance;

  factory DataSync() => instance ?? (instance = DataSync._());

  bool _isDownloading = false;

  bool get isDownloading => _isDownloading;

  bool _isUploading = false;

  bool get isUploading => _isUploading;

  bool _haveWaitingTask = false;

  String? _lastError;

  String? get lastError => _lastError;

  bool get isEnabled {
    var config = appdata.settings['webdav'];
    var autoSync = appdata.implicitData['webdavAutoSync'] ?? false;
    return autoSync && config is List && config.isNotEmpty;
  }

  Future<String> _options() async {
    final runtime = jsonDecode(await JsEngine.runtimeOptions());
    return jsonEncode({
      'directory': App.dataPath,
      'cache': App.cachePath,
      'http': runtime['http'],
    });
  }

  Future<Res<bool>> uploadData() async {
    if (_isDownloading || _haveWaitingTask) return const Res(true);
    if (_isUploading) {
      _haveWaitingTask = true;
      while (_isUploading) {
        await Future.delayed(const Duration(milliseconds: 100));
      }
      _haveWaitingTask = false;
    }
    _isUploading = true;
    _lastError = null;
    notifyListeners();
    try {
      await appdata.saveData(false);
      final data = jsonDecode(
        await backend.syncUpload(optionsJson: await _options()),
      );
      final revision = data['settings']['dataVersion'] as int;
      if (revision > (appdata.settings['dataVersion'] as int)) {
        appdata.settings['dataVersion'] = revision;
      }
      return const Res(true);
    } catch (e, s) {
      _lastError = e.toString();
      Log.error('Upload Data', e, s);
      return Res.error(e.toString());
    } finally {
      _isUploading = false;
      notifyListeners();
    }
  }

  Future<Res<bool>> downloadData() async {
    if (_haveWaitingTask) return const Res(true);
    _haveWaitingTask = true;
    while (_isDownloading || _isUploading) {
      await Future.delayed(const Duration(milliseconds: 100));
    }
    _haveWaitingTask = false;
    _isDownloading = true;
    _lastError = null;
    notifyListeners();
    try {
      final path = await backend.syncDownload(optionsJson: await _options());
      if (path != null) {
        final file = File(path);
        try {
          await importAppData(file, true);
        } finally {
          await file.deleteIgnoreError();
        }
      }
      return const Res(true);
    } catch (e, s) {
      _lastError = e.toString();
      Log.error('Download Data', e, s);
      return Res.error(e.toString());
    } finally {
      _isDownloading = false;
      notifyListeners();
    }
  }
}
