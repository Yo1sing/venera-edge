import 'dart:collection';
import 'dart:convert';
import 'package:venera/foundation/app.dart';
import 'package:venera/src/rust/api/script.dart' as backend;

/// Observable source data views write mutations directly to the Rust owner.
/// Nested maps/lists remain compatible with existing settings widgets.
class SourceDataMap extends MapBase<String, dynamic> {
  final String sourceKey;
  final List<String> path;
  SourceDataMap(this.sourceKey, [this.path = const []]);
  dynamic _read() => _readPath(sourceKey, path);
  @override
  dynamic operator [](Object? key) {
    final value = (_read() as Map)[key];
    return _wrap(sourceKey, [...path, key.toString()], value);
  }

  @override
  void operator []=(String key, dynamic value) =>
      _write(sourceKey, [...path, key], value);
  @override
  Iterable<String> get keys => List<String>.from((_read() as Map).keys);
  @override
  void clear() => _write(sourceKey, path, <String, dynamic>{});
  @override
  dynamic putIfAbsent(String key, dynamic Function() ifAbsent) {
    if (!containsKey(key)) this[key] = ifAbsent();
    return this[key];
  }

  @override
  dynamic remove(Object? key) {
    final value = (_read() as Map)[key];
    backend.updateSourceData(
      directory: App.dataPath,
      key: sourceKey,
      path: [...path, key.toString()],
    );
    return value;
  }
}

class _SourceDataList extends ListBase<dynamic> {
  final String sourceKey;
  final List<String> path;
  _SourceDataList(this.sourceKey, this.path);
  List<dynamic> _read() => List<dynamic>.from(_readPath(sourceKey, path));
  @override
  int get length => _read().length;
  @override
  set length(int value) {
    final list = _read()..length = value;
    _write(sourceKey, path, list);
  }

  @override
  dynamic operator [](int index) =>
      _wrap(sourceKey, [...path, '$index'], _read()[index]);
  @override
  void operator []=(int index, dynamic value) =>
      _write(sourceKey, [...path, '$index'], value);
}

dynamic _wrap(String key, List<String> path, dynamic value) {
  if (value is Map) return SourceDataMap(key, path);
  if (value is List) return _SourceDataList(key, path);
  return value;
}

dynamic _readPath(String key, List<String> path) {
  dynamic value = jsonDecode(
    backend.loadSourceData(directory: App.dataPath, key: key),
  );
  for (final part in path) {
    value = value is List ? value[int.parse(part)] : value[part];
  }
  return value;
}

void _write(String key, List<String> path, dynamic value) =>
    backend.updateSourceData(
      directory: App.dataPath,
      key: key,
      path: path,
      valueJson: jsonEncode(value),
    );
