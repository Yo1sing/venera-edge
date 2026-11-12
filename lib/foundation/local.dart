import 'dart:convert';
import 'dart:isolate';

import 'package:flutter/widgets.dart' show ChangeNotifier;
import 'package:flutter_saf/flutter_saf.dart';
import 'package:path_provider/path_provider.dart';
import 'package:venera/src/rust/api/local.dart' as backend;
import 'package:venera/foundation/comic_source/comic_source.dart';
import 'package:venera/foundation/comic_type.dart';
import 'package:venera/foundation/favorites.dart';
import 'package:venera/foundation/log.dart';
import 'package:venera/network/download.dart';
import 'package:venera/pages/reader/reader.dart';
import 'package:venera/utils/io.dart';

import 'app.dart';
import 'history.dart';

class LocalComic with HistoryMixin implements Comic {
  @override
  final String id;

  @override
  final String title;

  @override
  final String subtitle;

  @override
  final List<String> tags;

  /// The name of the directory where the comic is stored
  final String directory;

  /// key: chapter id, value: chapter title
  ///
  /// chapter id is the name of the directory in `LocalManager.path/$directory`
  final ComicChapters? chapters;

  bool get hasChapters => chapters != null;

  /// relative path to the cover image
  @override
  final String cover;

  final ComicType comicType;

  final List<String> downloadedChapters;

  final DateTime createdAt;

  const LocalComic({
    required this.id,
    required this.title,
    required this.subtitle,
    required this.tags,
    required this.directory,
    required this.chapters,
    required this.cover,
    required this.comicType,
    required this.downloadedChapters,
    required this.createdAt,
  });

  LocalComic.fromBackend(Map<String, dynamic> row)
    : id = row['id'],
      title = row['title'],
      subtitle = row['subtitle'],
      tags = List<String>.from(row['tags']),
      directory = row['directory'],
      chapters = ComicChapters.fromJsonOrNull(row['chapters']),
      cover = row['cover'],
      comicType = ComicType(row['comic_type']),
      downloadedChapters = List<String>.from(row['downloaded_chapters']),
      createdAt = DateTime.fromMillisecondsSinceEpoch(row['created_at']);

  Map<String, dynamic> toBackend() => {
    'id': id,
    'title': title,
    'subtitle': subtitle,
    'tags': tags,
    'directory': directory,
    'chapters': chapters?.toJson(),
    'cover': cover,
    'comic_type': comicType.value,
    'downloaded_chapters': downloadedChapters,
    'created_at': createdAt.millisecondsSinceEpoch,
  };

  File get coverFile => File(FilePath.join(baseDir, cover));

  String get baseDir => (directory.contains('/') || directory.contains('\\'))
      ? directory
      : FilePath.join(LocalManager().path, directory);

  @override
  String get description => "";

  @override
  String get sourceKey =>
      comicType == ComicType.local ? "local" : comicType.sourceKey;

  @override
  Map<String, dynamic> toJson() {
    return {
      "title": title,
      "cover": cover,
      "id": id,
      "subTitle": subtitle,
      "tags": tags,
      "description": description,
      "sourceKey": sourceKey,
      "chapters": chapters?.toJson(),
    };
  }

  @override
  int? get maxPage => null;

  void read() {
    var history = HistoryManager().find(id, comicType);
    int? firstDownloadedChapter;
    int? firstDownloadedChapterGroup;
    if (downloadedChapters.isNotEmpty && chapters != null) {
      final chapters = this.chapters!;
      if (chapters.isGrouped) {
        for (int i = 0; i < chapters.groupCount; i++) {
          var group = chapters.getGroupByIndex(i);
          var keys = group.keys.toList();
          for (int j = 0; j < keys.length; j++) {
            var chapterId = keys[j];
            if (downloadedChapters.contains(chapterId)) {
              firstDownloadedChapter = j + 1;
              firstDownloadedChapterGroup = i + 1;
              break;
            }
          }
        }
      } else {
        var keys = chapters.allChapters.keys;
        for (int i = 0; i < keys.length; i++) {
          if (downloadedChapters.contains(keys.elementAt(i))) {
            firstDownloadedChapter = i + 1;
            break;
          }
        }
      }
    }
    App.rootContext.to(
      () => Reader(
        type: comicType,
        cid: id,
        name: title,
        chapters: chapters,
        initialChapter: history?.ep ?? firstDownloadedChapter,
        initialPage: history?.page,
        initialChapterGroup: history?.group ?? firstDownloadedChapterGroup,
        history: history ?? History.fromModel(model: this, ep: 0, page: 0),
        author: subtitle,
        tags: tags,
      ),
    );
  }

  @override
  HistoryType get historyType => comicType;

  @override
  String? get subTitle => subtitle;

  @override
  String? get language => null;

  @override
  String? get favoriteId => null;

  @override
  double? get stars => null;
}

class LocalManager with ChangeNotifier {
  static LocalManager? _instance;

  LocalManager._();

  factory LocalManager() {
    return _instance ??= LocalManager._();
  }

  late backend.LocalStore _store;

  /// path to the directory where all the comics are stored
  late String path;

  Directory get directory => Directory(path);

  void _checkNoMedia() {
    if (App.isAndroid) {
      var file = File(FilePath.join(path, '.nomedia'));
      if (!file.existsSync()) {
        file.createSync();
      }
    }
  }

  // return error message if failed
  Future<String?> setNewPath(String newPath) async {
    var newDir = Directory(newPath);
    if (!await newDir.exists()) {
      return "Directory does not exist";
    }
    if (!await newDir.list().isEmpty) {
      return "Directory is not empty";
    }
    try {
      await copyDirectoryIsolate(directory, newDir);
      await File(
        FilePath.join(App.dataPath, 'local_path'),
      ).writeAsString(newPath);
    } catch (e, s) {
      Log.error("IO", e, s);
      return e.toString();
    }
    await directory.deleteContents(recursive: true);
    path = newPath;
    _checkNoMedia();
    return null;
  }

  Future<String> findDefaultPath() async {
    if (App.isAndroid) {
      var external = await getExternalStorageDirectories();
      if (external != null && external.isNotEmpty) {
        return FilePath.join(external.first.path, 'local');
      } else {
        return FilePath.join(App.dataPath, 'local');
      }
    } else if (App.isIOS) {
      var oldPath = FilePath.join(App.dataPath, 'local');
      if (Directory(oldPath).existsSync() &&
          Directory(oldPath).listSync().isNotEmpty) {
        return oldPath;
      } else {
        var directory = await getApplicationDocumentsDirectory();
        return FilePath.join(directory.path, 'local');
      }
    } else {
      return FilePath.join(App.dataPath, 'local');
    }
  }

  Future<void> _checkPathValidation() async {
    var testFile = File(FilePath.join(path, 'venera_test'));
    try {
      testFile.createSync();
      testFile.deleteSync();
    } catch (e) {
      Log.error(
        "IO",
        "Failed to create test file in local path: $e\nUsing default path instead.",
      );
      path = await findDefaultPath();
    }
  }

  Future<void> init() async {
    _store = await backend.LocalStore.open(path: '${App.dataPath}/local.db');
    if (File(FilePath.join(App.dataPath, 'local_path')).existsSync()) {
      path = File(FilePath.join(App.dataPath, 'local_path')).readAsStringSync();
      if (!directory.existsSync()) {
        path = await findDefaultPath();
      }
    } else {
      path = await findDefaultPath();
    }
    try {
      if (!directory.existsSync()) {
        await directory.create();
      }
    } catch (e, s) {
      Log.error("IO", "Failed to create local folder: $e", s);
    }
    await _checkPathValidation();
    _checkNoMedia();
    await ComicSourceManager().ensureInit();
    await restoreDownloadingTasks();
  }

  String findValidId(ComicType type) => _store.nextId(comicType: type.value);

  Future<void> add(LocalComic comic, [String? id]) async {
    _store.save(
      comicJson: jsonEncode({...comic.toBackend(), 'id': id ?? comic.id}),
    );
    notifyListeners();
  }

  void remove(String id, ComicType comicType) {
    _store.delete(
      keysJson: jsonEncode([
        {'id': id, 'type': comicType.value},
      ]),
    );
    notifyListeners();
  }

  void removeComic(LocalComic comic) => remove(comic.id, comic.comicType);
  List<LocalComic> _comics(String json) => (jsonDecode(json) as List)
      .map((row) => LocalComic.fromBackend(Map<String, dynamic>.from(row)))
      .toList();
  List<LocalComic> getComics(LocalSortType sortType) =>
      _comics(_store.list(sort: sortType.value));
  LocalComic? find(String id, ComicType comicType) {
    final json = _store.find(id: id, comicType: comicType.value);
    return json == null ? null : LocalComic.fromBackend(jsonDecode(json));
  }

  @override
  void dispose() {
    _store.close();
    _store.dispose();
    super.dispose();
  }

  List<LocalComic> getRecent() =>
      _comics(_store.list(sort: 'time_desc', limit: 20));
  int get count => _store.count();
  LocalComic? findByName(String name) {
    final json = _store.findName(name: name);
    return json == null ? null : LocalComic.fromBackend(jsonDecode(json));
  }

  List<LocalComic> search(String keyword) =>
      _comics(_store.list(sort: 'time_desc', keyword: keyword));

  Future<List<String>> getImages(String id, ComicType type, Object ep) async {
    if (ep is! String && ep is! int) {
      throw "Invalid ep";
    }
    var comic = find(id, type) ?? (throw "Comic Not Found");
    var directory = Directory(comic.baseDir);
    if (comic.hasChapters) {
      var cid = ep is int
          ? comic.chapters!.ids.elementAt(ep - 1)
          : (ep as String);
      cid = getChapterDirectoryName(cid);
      directory = Directory(FilePath.join(directory.path, cid));
    }
    if (directory is! AndroidDirectory) {
      return backend.listComicImages(path: directory.path);
    }
    final files = await directory
        .list()
        .where((e) => e is File)
        .map((e) => e.path)
        .toList();
    return backend.sortComicImages(paths: files);
  }

  bool isDownloaded(
    String id,
    ComicType type, [
    int? ep,
    ComicChapters? chapters,
  ]) => _store.downloaded(
    id: id,
    comicType: type.value,
    episode: ep,
    chaptersJson: chapters == null ? null : jsonEncode(chapters.toJson()),
  );

  List<DownloadTask> downloadingTasks = [];

  bool isDownloading(String id, ComicType type) {
    return downloadingTasks.any(
      (element) => element.id == id && element.comicType == type,
    );
  }

  Future<Directory> findValidDirectory(
    String id,
    ComicType type,
    String name,
  ) async {
    var comic = find(id, type);
    if (comic != null) {
      return Directory(FilePath.join(path, comic.directory));
    }
    const comicDirectoryMaxLength = 80;
    if (name.length > comicDirectoryMaxLength) {
      name = name.substring(0, comicDirectoryMaxLength);
    }
    var dir = findValidDirectoryName(path, name);
    return Directory(FilePath.join(path, dir)).create().then((value) => value);
  }

  void applyDownloadSnapshot(List<DownloadTask> tasks) {
    downloadingTasks = tasks;
    notifyListeners();
  }

  void removeTask(DownloadTask task) => task.cancel();
  void moveToFirst(DownloadTask task) => DownloadQueueView.command(task.queueKey, 'first');
  Future<void> restoreDownloadingTasks() => DownloadQueueView.initialize();
  void addTask(DownloadTask task) => DownloadQueueView.add(task);

  void deleteComic(LocalComic c, [bool removeFileOnDisk = true]) {
    if (removeFileOnDisk) {
      var dir = Directory(FilePath.join(path, c.directory));
      dir.deleteIgnoreError(recursive: true);
    }
    // Deleting a local comic means that it's no longer available, thus both favorite and history should be deleted.
    if (c.comicType == ComicType.local) {
      if (HistoryManager().find(c.id, c.comicType) != null) {
        HistoryManager().remove(c.id, c.comicType);
      }
      var folders = LocalFavoritesManager().find(c.id, c.comicType);
      for (var f in folders) {
        LocalFavoritesManager().deleteComicWithId(f, c.id, c.comicType);
      }
    }
    remove(c.id, c.comicType);
    notifyListeners();
  }

  void deleteComicChapters(LocalComic c, List<String> chapters) {
    if (chapters.isEmpty) {
      return;
    }
    _store.deleteChapters(
      id: c.id,
      comicType: c.comicType.value,
      chapters: chapters,
    );
    var shouldRemovedDirs = <Directory>[];
    for (var chapter in chapters) {
      var dir = Directory(
        FilePath.join(c.baseDir, getChapterDirectoryName(chapter)),
      );
      if (dir.existsSync()) {
        shouldRemovedDirs.add(dir);
      }
    }
    if (shouldRemovedDirs.isNotEmpty) {
      _deleteDirectories(shouldRemovedDirs);
    }
    notifyListeners();
  }

  void batchDeleteComics(
    List<LocalComic> comics, [
    bool removeFileOnDisk = true,
    bool removeFavoriteAndHistory = true,
  ]) {
    if (comics.isEmpty) {
      return;
    }

    var shouldRemovedDirs = <Directory>[];
    _store.delete(
      keysJson: jsonEncode(
        comics.map((c) => {'id': c.id, 'type': c.comicType.value}).toList(),
      ),
    );
    if (removeFileOnDisk) {
      for (final c in comics) {
        final dir = Directory(c.baseDir);
        if (dir.existsSync()) shouldRemovedDirs.add(dir);
      }
    }

    var comicIDs = comics.map((e) => ComicID(e.comicType, e.id)).toList();

    if (removeFavoriteAndHistory) {
      LocalFavoritesManager().batchDeleteComicsInAllFolders(comicIDs);
      HistoryManager().batchDeleteHistories(comicIDs);
    }

    notifyListeners();

    if (removeFileOnDisk) {
      _deleteDirectories(shouldRemovedDirs);
    }
  }

  /// Deletes the directories in a separate isolate to avoid blocking the UI thread.
  static void _deleteDirectories(List<Directory> directories) {
    Isolate.run(() async {
      await SAFTaskWorker().init();
      for (var dir in directories) {
        try {
          if (dir.existsSync()) {
            await dir.delete(recursive: true);
          }
        } catch (e) {
          continue;
        }
      }
    });
  }

  static String getChapterDirectoryName(String name) =>
      backend.chapterDirectoryName(name: name);
}

enum LocalSortType {
  name("name"),
  timeAsc("time_asc"),
  timeDesc("time_desc");

  final String value;

  const LocalSortType(this.value);

  static LocalSortType fromString(String value) {
    for (var type in values) {
      if (type.value == value) {
        return type;
      }
    }
    return name;
  }
}
