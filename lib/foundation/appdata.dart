import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:uuid/uuid.dart';
import 'package:venera/foundation/app.dart';
import 'package:venera/foundation/log.dart';
import 'package:venera/src/rust/api/settings.dart' as backend;
import 'package:venera/utils/data_sync.dart';
import 'package:venera/utils/init.dart';

/// Observable UI snapshot; persistence and synchronization rules live in Rust.
class Appdata with Init {
  Appdata._create();
  final Settings settings = Settings._create();
  var searchHistory = <String>[];
  var implicitData = <String, dynamic>{};
  Future<void> _pendingSave = Future.value();

  Future<void> saveData([bool sync = true]) async {
    final snapshot = jsonEncode(toJson());
    final operation = _pendingSave.then(
      (_) => backend.saveAppData(directory: App.dataPath, dataJson: snapshot),
    );
    _pendingSave = operation.catchError((Object error, StackTrace stack) {
      Log.error('Appdata', error, stack);
    });
    await operation;
    if (sync) DataSync().uploadData();
  }

  void addSearchHistory(String keyword) {
    searchHistory = backend.updateSearchHistory(
      history: searchHistory,
      keyword: keyword,
      remove: false,
    );
    unawaited(saveData());
  }

  void removeSearchHistory(String keyword) {
    searchHistory = backend.updateSearchHistory(
      history: searchHistory,
      keyword: keyword,
      remove: true,
    );
    unawaited(saveData());
  }

  void clearSearchHistory() {
    searchHistory = backend.updateSearchHistory(
      history: searchHistory,
      remove: true,
    );
    unawaited(saveData());
  }

  Map<String, dynamic> toJson() => {
    'settings': settings._data,
    'searchHistory': searchHistory,
  };

  void _apply(Map<String, dynamic> data) {
    settings._replace(Map<String, dynamic>.from(data['settings']));
    searchHistory = List<String>.from(data['searchHistory']);
  }

  void syncData(Map<String, dynamic> data) {
    _apply(
      jsonDecode(
        backend.mergeSyncedData(
          currentJson: jsonEncode(toJson()),
          incomingJson: jsonEncode(data),
        ),
      ),
    );
    unawaited(saveData());
  }

  void writeImplicitData() {
    final snapshot = jsonEncode(implicitData);
    _pendingSave = _pendingSave
        .then(
          (_) => backend.saveImplicitData(
            directory: App.dataPath,
            dataJson: snapshot,
          ),
        )
        .catchError((Object error, StackTrace stack) {
          Log.error('Appdata', error, stack);
        });
  }

  @override
  Future<void> doInit() async {
    try {
      final data = jsonDecode(
        await backend.loadAppData(directory: App.dataPath),
      );
      _apply(data);
      implicitData = Map<String, dynamic>.from(data['implicitData']);
      await saveData(false);
    } catch (error, stack) {
      // Keep corrupt/unreadable files for recovery, and let other components initialize.
      Log.error('Appdata', error, stack);
    }
  }
}

final appdata = Appdata._create();

class Settings with ChangeNotifier {
  Settings._create();

  final Map<String, dynamic> _data = jsonDecode(backend.defaultSettingsJson());

  void _replace(Map<String, dynamic> values) {
    _data.addAll(values);
    notifyListeners();
  }

  operator [](String key) {
    return _data[key];
  }

  operator []=(String key, dynamic value) {
    _data[key] = value;
    if (key != "dataVersion") {
      notifyListeners();
    }
  }

  void setEnabledComicSpecificSettings(
    String comicId,
    String sourceKey,
    bool enabled,
  ) {
    setReaderSetting(comicId, sourceKey, "enabled", enabled);
  }

  bool isComicSpecificSettingsEnabled(String? comicId, String? sourceKey) {
    if (comicId == null || sourceKey == null) {
      return false;
    }
    return _data['comicSpecificSettings']["$comicId@$sourceKey"]?["enabled"] ==
        true;
  }

  dynamic getReaderSetting(String comicId, String sourceKey, String key) {
    return jsonDecode(
      backend.readerSetting(
        settingsJson: jsonEncode(_data),
        comicKey: '$comicId@$sourceKey',
        key: key,
      ),
    );
  }

  void setReaderSetting(
    String comicId,
    String sourceKey,
    String key,
    dynamic value,
  ) {
    (_data['comicSpecificSettings'] as Map<String, dynamic>).putIfAbsent(
      "$comicId@$sourceKey",
      () => <String, dynamic>{},
    )[key] = value;
    notifyListeners();
  }

  void resetComicReaderSettings(String key) {
    (_data['comicSpecificSettings'] as Map).remove(key);
    notifyListeners();
  }

  void setEnabledDeviceSpecificSettings(bool enabled) {
    setDeviceReaderSetting("enabled", enabled);
  }

  bool isDeviceSpecificSettingsEnabled() {
    var deviceId = _data['deviceId'] as String;
    if (deviceId.isEmpty) {
      return false;
    }
    return _data['deviceSpecificSettings'][deviceId]?["enabled"] == true;
  }

  dynamic getDeviceReaderSetting(String key) {
    return jsonDecode(
      backend.readerSetting(settingsJson: jsonEncode(_data), key: key),
    );
  }

  void setDeviceReaderSetting(String key, dynamic value) {
    var deviceId = _getOrCreateDeviceId();
    (_data['deviceSpecificSettings'] as Map<String, dynamic>).putIfAbsent(
      deviceId,
      () => <String, dynamic>{},
    )[key] = value;
    notifyListeners();
  }

  void resetDeviceReaderSettings() {
    var deviceId = _data['deviceId'] as String;
    if (deviceId.isEmpty) {
      return;
    }
    (_data['deviceSpecificSettings'] as Map).remove(deviceId);
    notifyListeners();
  }

  String _getOrCreateDeviceId() {
    var deviceId = _data['deviceId'] as String;
    if (deviceId.isNotEmpty) {
      return deviceId;
    }
    var id = const Uuid().v4();
    _data['deviceId'] = id;
    return id;
  }

  @override
  String toString() {
    return _data.toString();
  }
}

String get defaultCustomImageProcessing =>
    jsonDecode(backend.defaultSettingsJson())['customImageProcessing']
        as String;
