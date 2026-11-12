import 'dart:convert';
import 'package:venera/src/rust/api/favorites.dart' as backend;

import 'package:flutter/foundation.dart';
import 'package:venera/foundation/appdata.dart';
import 'package:venera/foundation/image_provider/local_favorite_image.dart';
import 'package:venera/foundation/local.dart';
import 'package:venera/pages/follow_updates_page.dart';

import 'app.dart';
import 'comic_source/comic_source.dart';
import 'comic_type.dart';

String _getTimeString(DateTime time) {
  return time.toIso8601String().replaceFirst("T", " ").substring(0, 19);
}

class FavoriteItem implements Comic {
  String name;
  String author;
  ComicType type;
  @override
  List<String> tags;
  @override
  String id;
  String coverPath;
  late String time;

  FavoriteItem({
    required this.id,
    required this.name,
    required this.coverPath,
    required this.author,
    required this.type,
    required this.tags,
    DateTime? favoriteTime,
  }) {
    var t = favoriteTime ?? DateTime.now();
    time = _getTimeString(t);
  }

  FavoriteItem.fromRow(Map<String, dynamic> row)
    : name = row["name"],
      author = row["author"],
      type = ComicType(row["type"]),
      tags = List<String>.from(row["tags"]),
      id = row["id"],
      coverPath = row["coverPath"],
      time = row["time"] {
    tags.remove("");
  }

  @override
  bool operator ==(Object other) {
    return other is FavoriteItem && other.id == id && other.type == type;
  }

  @override
  int get hashCode => id.hashCode ^ type.hashCode;

  @override
  String toString() {
    var s = "FavoriteItem: $name $author $coverPath $hashCode $tags";
    if (s.length > 100) {
      return s.substring(0, 100);
    }
    return s;
  }

  @override
  String get cover => coverPath;

  @override
  String get description {
    var time = this.time.substring(0, 10);
    return appdata.settings['comicDisplayMode'] == 'detailed'
        ? "$time | ${type == ComicType.local ? 'local' : type.comicSource?.name ?? "Unknown"}"
        : "${type.comicSource?.name ?? "Unknown"} | $time";
  }

  @override
  String? get favoriteId => null;

  @override
  String? get language => null;

  @override
  int? get maxPage => null;

  @override
  String get sourceKey => type == ComicType.local
      ? 'local'
      : type.comicSource?.key ?? "Unknown:${type.value}";

  @override
  double? get stars => null;

  @override
  String? get subtitle => author;

  @override
  String get title => name;

  @override
  Map<String, dynamic> toJson() {
    return {
      "name": name,
      "author": author,
      "type": type.value,
      "tags": tags,
      "id": id,
      "coverPath": coverPath,
    };
  }

  static FavoriteItem fromJson(Map<String, dynamic> json) {
    var type = json["type"] as int;
    if (type == 0 && json['coverPath'].toString().startsWith('http')) {
      type = 'picacg'.hashCode;
    } else if (type == 1) {
      type = 'ehentai'.hashCode;
    } else if (type == 2) {
      type = 'jm'.hashCode;
    } else if (type == 3) {
      type = 'hitomi'.hashCode;
    } else if (type == 4) {
      type = 'wnacg'.hashCode;
    } else if (type == 6) {
      type = 'nhentai'.hashCode;
    }
    return FavoriteItem(
      id: json["id"] ?? json['target'],
      name: json["name"],
      author: json["author"],
      coverPath: json["coverPath"],
      type: ComicType(type),
      tags: List<String>.from(json["tags"] ?? []),
    );
  }
}

class FavoriteItemWithFolderInfo extends FavoriteItem {
  String folder;

  FavoriteItemWithFolderInfo(FavoriteItem item, this.folder)
    : super(
        id: item.id,
        name: item.name,
        coverPath: item.coverPath,
        author: item.author,
        type: item.type,
        tags: item.tags,
      );
}

class FavoriteItemWithUpdateInfo extends FavoriteItem {
  String? updateTime;

  DateTime? lastCheckTime;

  bool hasNewUpdate;

  FavoriteItemWithUpdateInfo(
    FavoriteItem item,
    this.updateTime,
    this.hasNewUpdate,
    int? lastCheckTime,
  ) : lastCheckTime = lastCheckTime == null
          ? null
          : DateTime.fromMillisecondsSinceEpoch(lastCheckTime),
      super(
        id: item.id,
        name: item.name,
        coverPath: item.coverPath,
        author: item.author,
        type: item.type,
        tags: item.tags,
      );

  @override
  String get description {
    var updateTime = this.updateTime ?? "Unknown";
    var sourceName = type.comicSource?.name ?? "Unknown";
    return "$updateTime | $sourceName";
  }

  @override
  operator ==(Object other) {
    return other is FavoriteItemWithUpdateInfo &&
        other.updateTime == updateTime &&
        other.hasNewUpdate == hasNewUpdate &&
        super == other;
  }

  @override
  int get hashCode =>
      super.hashCode ^ updateTime.hashCode ^ hasNewUpdate.hashCode;
}

/// UI compatibility adapter. The Rust store owns the database and mutations.
class LocalFavoritesManager with ChangeNotifier {
  factory LocalFavoritesManager() => cache ??= LocalFavoritesManager._create();
  LocalFavoritesManager._create();
  static LocalFavoritesManager? cache;
  late backend.FavoriteStore _store;
  Map<String, int> counts = {};
  int _total = 0;
  bool get _traditional => App.locale.countryCode == 'TW';
  bool get _atEnd => appdata.settings['newFavoriteAddTo'] == 'end';
  int get totalComics => _total;
  int folderComics(String folder) => counts[folder] ?? 0;
  List<String> get folderNames => _store.folders();

  Future<void> init() async {
    _store = await backend.FavoriteStore.open(
      path: '${App.dataPath}/local_favorite.db',
      traditional: _traditional,
    );
    await appdata.ensureInit();
    final follow = appdata.settings['followUpdatesFolder'];
    if (follow is String && existsFolder(follow)) {
      prepareTableForFollowUpdates(follow, false);
    } else {
      appdata.settings['followUpdatesFolder'] = null;
    }
    initCounts();
  }

  void initCounts() {
    final data = jsonDecode(_store.summary());
    counts = Map<String, int>.from(data['counts']);
    _total = data['total'];
    notifyListeners();
  }

  void refreshHashedIds() => initCounts();
  void notifyChanges() => notifyListeners();
  List<FavoriteItem> _items(String json) => (jsonDecode(json) as List)
      .map((row) => FavoriteItem.fromRow(Map<String, dynamic>.from(row)))
      .toList();
  String _comic(FavoriteItem comic, [String? updateTime]) => jsonEncode({
    ...comic.toJson(),
    'time': comic.time,
    'last_update_time': updateTime,
  });
  String _keys(Iterable<FavoriteItem> comics) => jsonEncode(
    comics.map((c) => {'id': c.id, 'type': c.type.value}).toList(),
  );
  String _key(String id, ComicType type) => jsonEncode([
    {'id': id, 'type': type.value},
  ]);
  List<String> find(String id, ComicType type) =>
      _store.find(id: id, comicType: type.value);
  Future<List<String>> findWithModel(FavoriteItem item) async =>
      find(item.id, item.type);
  bool isExist(String id, ComicType type) => find(id, type).isNotEmpty;
  bool existsFolder(String name) => folderNames.contains(name);
  int count(String folderName) =>
      (jsonDecode(_store.summary())['counts'][folderName] as int?) ?? 0;
  int maxValue(String folder) => _store.bound(folder: folder, maximum: true);
  int minValue(String folder) => _store.bound(folder: folder, maximum: false);
  List<FavoriteItem> getFolderComics(String folder) =>
      _items(_store.list(folder: folder, onlyUpdates: false));
  Future<List<FavoriteItem>> getFolderComicsAsync(String folder) async =>
      _items(await _store.listAsync(folder: folder));
  List<FavoriteItem> getAllComics() => _items(_store.allUnique());
  Future<List<FavoriteItem>> getAllComicsAsync() async =>
      _items(await _store.allUniqueAsync());
  List<FavoriteItemWithFolderInfo> allComics() =>
      (jsonDecode(_store.list(onlyUpdates: false)) as List)
          .map(
            (row) => FavoriteItemWithFolderInfo(
              FavoriteItem.fromRow(Map<String, dynamic>.from(row)),
              row['folder'],
            ),
          )
          .toList();
  void updateOrder(List<String> folders) {
    _store.orderFolders(folders: folders);
    notifyListeners();
  }

  String createFolder(String name, [bool renameWhenInvalidName = false]) {
    final result = _store.create(
      name: name,
      renameInvalid: renameWhenInvalidName,
    );
    initCounts();
    return result;
  }

  void linkFolderToNetwork(
    String folder,
    String source,
    String networkFolder,
  ) =>
      _store.link(folder: folder, source: source, networkFolder: networkFolder);
  (String?, String?) findLinked(String folder) {
    final result = _store.linked(folder: folder);
    return result.isEmpty ? (null, null) : (result[0], result[1]);
  }

  bool isLinkedToNetworkFolder(
    String folder,
    String source,
    String networkFolder,
  ) => findLinked(folder) == (source, networkFolder);
  bool comicExists(String folder, String id, ComicType type) =>
      _store.get_(folder: folder, id: id, comicType: type.value) != null;
  FavoriteItem getComic(String folder, String id, ComicType type) {
    final data = _store.get_(folder: folder, id: id, comicType: type.value);
    if (data == null) throw StateError('Comic not found');
    return FavoriteItem.fromRow(jsonDecode(data));
  }

  bool addComic(
    String folder,
    FavoriteItem comic, [
    int? order,
    String? updateTime,
  ]) {
    final added = _store.add(
      folder: folder,
      comicJson: _comic(comic, updateTime),
      order: order,
      atEnd: _atEnd,
      traditional: _traditional,
    );
    if (added) initCounts();
    return added;
  }

  void moveFavorite(
    String sourceFolder,
    String targetFolder,
    String id,
    ComicType type,
  ) {
    _store.transfer(
      source: sourceFolder,
      target: targetFolder,
      keysJson: _key(id, type),
      copy: false,
      atEnd: false,
      keepDuplicate: true,
    );
    initCounts();
  }

  void batchMoveFavorites(
    String sourceFolder,
    String targetFolder,
    List<FavoriteItem> items,
  ) {
    _store.transfer(
      source: sourceFolder,
      target: targetFolder,
      keysJson: _keys(items),
      copy: false,
      atEnd: true,
      keepDuplicate: false,
    );
    initCounts();
  }

  void batchCopyFavorites(
    String sourceFolder,
    String targetFolder,
    List<FavoriteItem> items,
  ) {
    _store.transfer(
      source: sourceFolder,
      target: targetFolder,
      keysJson: _keys(items),
      copy: true,
      atEnd: true,
      keepDuplicate: false,
    );
    initCounts();
  }

  void deleteFolder(String name) {
    _store.deleteFolder(folder: name);
    initCounts();
  }

  void deleteComicWithId(String folder, String id, ComicType type) {
    _store.delete(folder: folder, keysJson: _key(id, type));
    LocalFavoriteImageProvider.delete(id, type.value);
    initCounts();
  }

  void batchDeleteComics(String folder, List<FavoriteItem> comics) {
    _store.delete(folder: folder, keysJson: _keys(comics));
    for (final comic in comics) {
      LocalFavoriteImageProvider.delete(comic.id, comic.type.value);
    }
    initCounts();
  }

  void batchDeleteComicsInAllFolders(List<ComicID> comics) {
    _store.delete(
      keysJson: jsonEncode(
        comics.map((c) => {'id': c.id, 'type': c.type.value}).toList(),
      ),
    );
    for (final comic in comics) {
      LocalFavoriteImageProvider.delete(comic.id, comic.type.value);
    }
    initCounts();
  }

  Future<int> removeInvalid() async {
    // Source availability still comes from the source runtime during migration.
    var removed = 0;
    for (final comic in allComics()) {
      if ((comic.type == ComicType.local &&
              LocalManager().find(comic.id, comic.type) == null) ||
          (comic.type != ComicType.local && comic.type.comicSource == null)) {
        deleteComicWithId(comic.folder, comic.id, comic.type);
        removed++;
      }
    }
    return removed;
  }

  Future<void> clearAll() async {
    _store.clear();
    appdata.settings['followUpdatesFolder'] = null;
    initCounts();
  }

  void reorder(List<FavoriteItem> newFolder, String folder) {
    _store.reorder(folder: folder, keysJson: _keys(newFolder));
    notifyListeners();
  }

  void rename(String before, String after) {
    _store.rename(before: before, after: after);
    initCounts();
  }

  void onRead(String id, ComicType type) {
    _store.onRead(
      id: id,
      comicType: type.value,
      position: appdata.settings['moveFavoriteAfterRead'] ?? 'none',
      followFolder: appdata.settings['followUpdatesFolder'],
    );
    updateFollowUpdatesUI();
    notifyListeners();
  }

  void markAsRead(String id, ComicType type) {
    _store.onRead(
      id: id,
      comicType: type.value,
      position: 'none',
      followFolder: appdata.settings['followUpdatesFolder'],
    );
  }

  List<FavoriteItem> searchInFolder(String folder, String keyword) =>
      _items(_store.list(folder: folder, keyword: keyword, onlyUpdates: false));
  List<FavoriteItem> search(String keyword) =>
      _items(_store.allUnique(keyword: keyword));
  void editTags(String id, String folder, List<String> tags) {
    _store.editTags(
      folder: folder,
      id: id,
      tags: tags,
      prepend: false,
      traditional: _traditional,
    );
    notifyListeners();
  }

  void addTagTo(String folder, String id, String tag) {
    _store.editTags(
      folder: folder,
      id: id,
      tags: [tag],
      prepend: true,
      traditional: _traditional,
    );
    notifyListeners();
  }

  void updateInfo(String folder, FavoriteItem comic, [bool notify = true]) {
    _store.updateInfo(
      folder: folder,
      comicJson: _comic(comic),
      traditional: _traditional,
    );
    if (notify) notifyListeners();
  }

  String folderToJson(String folder) => _store.export_(folder: folder);
  void fromJson(String json) {
    _store.import_(
      text: json,
      atEnd: _atEnd,
      traditional: _traditional,
      legacyTypes: {
        0: 'picacg'.hashCode,
        1: 'ehentai'.hashCode,
        2: 'jm'.hashCode,
        3: 'hitomi'.hashCode,
        4: 'wnacg'.hashCode,
        6: 'nhentai'.hashCode,
      },
    );
    initCounts();
  }

  void prepareTableForFollowUpdates(String table, [bool clearData = true]) =>
      _store.prepareFollow(folder: table, clear: clearData);
  void updateUpdateTime(
    String folder,
    String id,
    ComicType type,
    String updateTime,
  ) => _store.updateCheck(
    folder: folder,
    id: id,
    comicType: type.value,
    updateTime: updateTime,
  );
  void updateCheckTime(String folder, String id, ComicType type) =>
      _store.updateCheck(folder: folder, id: id, comicType: type.value);
  int countUpdates(String folder) => getUpdates(folder).length;
  List<FavoriteItemWithUpdateInfo> _updates(String folder, bool onlyUpdates) {
    if (!existsFolder(folder)) return [];
    return (jsonDecode(_store.list(folder: folder, onlyUpdates: onlyUpdates))
            as List)
        .map(
          (row) => FavoriteItemWithUpdateInfo(
            FavoriteItem.fromRow(Map<String, dynamic>.from(row)),
            row['last_update_time'],
            row['has_new_update'],
            row['last_check_time'],
          ),
        )
        .toList();
  }

  List<FavoriteItemWithUpdateInfo> getUpdates(String folder) =>
      _updates(folder, true);
  List<FavoriteItemWithUpdateInfo> getComicsWithUpdatesInfo(String folder) =>
      _updates(folder, false);
  void close() {
    _store.close();
    _store.dispose();
  }
}
