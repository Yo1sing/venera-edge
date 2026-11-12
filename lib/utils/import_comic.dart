import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:venera/components/components.dart';
import 'package:venera/foundation/app.dart';
import 'package:venera/foundation/comic_source/comic_source.dart';
import 'package:venera/foundation/comic_type.dart';
import 'package:venera/foundation/favorites.dart';
import 'package:venera/foundation/local.dart';
import 'package:venera/foundation/log.dart';
import 'package:venera/src/rust/api/archive.dart' as archive_backend;
import 'package:venera/utils/ext.dart';
import 'package:venera/utils/translations.dart';
import 'cbz.dart';
import 'io.dart';

class ImportComic {
  final String? selectedFolder;
  final bool copyToLocal;

  const ImportComic({this.selectedFolder, this.copyToLocal = true});

  Future<bool> cbz() async {
    var file = await selectFile(ext: ['cbz', 'zip', '7z', 'cb7']);
    Map<String?, List<LocalComic>> imported = {};
    if (file == null) {
      return false;
    }
    var controller = showLoadingDialog(App.rootContext, allowCancel: false);
    try {
      var comic = await CBZ.import(File(file.path));
      imported[selectedFolder] = [comic];
    } catch (e, s) {
      Log.error("Import Comic", e.toString(), s);
      App.rootContext.showMessage(message: e.toString());
    }
    controller.close();
    return registerComics(imported, false);
  }

  Future<bool> multipleCbz() async {
    var picker = DirectoryPicker();
    var dir = await picker.pickDirectory(directAccess: true);
    if (dir != null) {
      var files = (await dir.list().toList()).whereType<File>().toList();
      const supportedExtensions = ['cbz', 'zip', '7z', 'cb7'];
      files.removeWhere((e) => !supportedExtensions.contains(e.extension));
      Map<String?, List<LocalComic>> imported = {};
      var controller = showLoadingDialog(App.rootContext, allowCancel: false);
      var comics = <LocalComic>[];
      for (var file in files) {
        try {
          var comic = await CBZ.import(file);
          comics.add(comic);
        } catch (e, s) {
          Log.error("Import Comic", e.toString(), s);
        }
      }
      if (comics.isEmpty) {
        App.rootContext.showMessage(message: "No valid comics found".tl);
      }
      imported[selectedFolder] = comics;
      controller.close();
      return registerComics(imported, false);
    }
    return false;
  }

  Future<bool> ehViewer() async {
    var dbFile = await selectFile(ext: ['db']);
    final picker = DirectoryPicker();
    final comicSrc = await picker.pickDirectory();
    Map<String?, List<LocalComic>> imported = {};
    if (dbFile == null || comicSrc == null) {
      return false;
    }

    bool cancelled = false;
    var controller = showLoadingDialog(
      App.rootContext,
      onCancel: () {
        cancelled = true;
      },
    );

    try {
      final groups =
          jsonDecode(
                await archive_backend.readEhviewerLibrary(
                  database: dbFile.path,
                ),
              )
              as List;

      Future<List<LocalComic>> validateComics(List<dynamic> comics) async {
        List<LocalComic> imported = [];
        for (var comic in comics) {
          if (cancelled) {
            return imported;
          }
          final comicDir = Directory(
            FilePath.join(comicSrc.path, comic['directory']),
          );
          final comicObj = await _checkSingleComic(
            comicDir,
            title: comic['title'],
            tags: List<String>.from(comic['tags']),
            createTime: DateTime.fromMillisecondsSinceEpoch(
              comic['created_at'],
            ),
          );
          if (comicObj == null) {
            continue;
          }
          imported.add(comicObj);
        }
        return imported;
      }

      for (final group in groups) {
        if (cancelled) break;
        final String tag = group['label'];
        final folderName = tag.isEmpty
            ? '(EhViewer)Default'.tl
            : '(EhViewer)$tag';
        final comicList = group['comics'] as List;
        var validComics = await validateComics(comicList);
        imported[folderName] = validComics;
        if (validComics.isNotEmpty &&
            !LocalFavoritesManager().existsFolder(folderName)) {
          LocalFavoritesManager().createFolder(folderName);
        }
      }

      //Android specific
      var cache = FilePath.join(App.cachePath, dbFile.name);
      await File(cache).deleteIgnoreError();
    } catch (e, s) {
      Log.error("Import Comic", e.toString(), s);
      App.rootContext.showMessage(message: e.toString());
    }
    controller.close();
    if (cancelled) return false;
    return registerComics(imported, copyToLocal);
  }

  Future<bool> directory(bool single) async {
    final picker = DirectoryPicker();
    final path = await picker.pickDirectory();
    if (path == null) {
      return false;
    }
    Map<String?, List<LocalComic>> imported = {selectedFolder: []};
    try {
      if (single) {
        var result = await _checkSingleComic(path);
        if (result != null) {
          imported[selectedFolder]!.add(result);
        } else {
          App.rootContext.showMessage(message: "Invalid Comic".tl);
          return false;
        }
      } else {
        await for (var entry in path.list()) {
          if (entry is Directory) {
            var result = await _checkSingleComic(entry);
            if (result != null) {
              imported[selectedFolder]!.add(result);
            }
          }
        }
      }
    } catch (e, s) {
      Log.error("Import Comic", e.toString(), s);
      App.rootContext.showMessage(message: e.toString());
    }
    return registerComics(imported, copyToLocal);
  }

  Future<bool> localDownloads() async {
    var localDir = LocalManager().directory;
    Map<String?, List<LocalComic>> imported = {null: []};
    bool cancelled = false;
    var controller = showLoadingDialog(
      App.rootContext,
      onCancel: () {
        cancelled = true;
      },
    );
    try {
      if (!await localDir.exists()) {
        App.rootContext.showMessage(message: "Local path not found".tl);
        controller.close();
        return false;
      }
      await for (var entry in localDir.list()) {
        if (cancelled) {
          break;
        }
        if (entry is Directory) {
          var stat = await entry.stat();
          var result = await _checkSingleComic(
            entry,
            createTime: stat.modified,
            useRelativePath: true,
          );
          if (result != null) {
            imported[null]!.add(result);
          }
        }
      }
      if (!cancelled && imported[null]!.isEmpty) {
        App.rootContext.showMessage(message: "No valid comics found".tl);
      }
    } catch (e, s) {
      Log.error("Import Comic", e.toString(), s);
      App.rootContext.showMessage(message: e.toString());
    }
    controller.close();
    if (cancelled) return false;
    return registerComics(imported, false);
  }

  //Automatically search for cover image and chapters
  Future<LocalComic?> _checkSingleComic(
    Directory directory, {
    String? id,
    String? title,
    String? subtitle,
    List<String>? tags,
    DateTime? createTime,
    bool useRelativePath = false,
  }) async {
    if (!(await directory.exists())) return null;
    var name = title ?? directory.name;
    if (LocalManager().findByName(name) != null) {
      Log.info("Import Comic", "Comic already exists: $name");
      return null;
    }
    bool hasChapters = false;
    var chapters = <String>[];
    var coverPath = ''; // relative path to the cover image
    var fileList = <String>[];
    await for (var entry in directory.list()) {
      if (entry is Directory) {
        hasChapters = true;
        chapters.add(entry.name);
        await for (var file in entry.list()) {
          if (file is Directory) {
            Log.info(
              "Import Comic",
              "Invalid Chapter: ${entry.name}\nA directory is found in the chapter directory.",
            );
            return null;
          }
        }
      } else if (entry is File) {
        const imageExtensions = ['jpg', 'jpeg', 'png', 'webp', 'gif', 'jpe'];
        if (imageExtensions.contains(entry.extension)) {
          fileList.add(entry.name);
        }
      }
    }

    if (fileList.isEmpty) {
      return null;
    }

    fileList.sort();
    coverPath =
        fileList.firstWhereOrNull((l) => l.startsWith('cover')) ??
        fileList.first;

    chapters.sort();
    if (hasChapters && coverPath == '') {
      // use the first image in the first chapter as the cover
      var firstChapter = Directory('${directory.path}/${chapters.first}');
      await for (var entry in firstChapter.list()) {
        if (entry is File) {
          coverPath = entry.name;
          break;
        }
      }
    }
    if (coverPath == '') {
      Log.info("Import Comic", "Invalid Comic: $name\nNo cover image found.");
      return null;
    }
    var directoryPath = useRelativePath ? directory.name : directory.path;
    return LocalComic(
      id: id ?? '0',
      title: name,
      subtitle: subtitle ?? '',
      tags: tags ?? [],
      directory: directoryPath,
      chapters: hasChapters
          ? ComicChapters(Map.fromIterables(chapters, chapters))
          : null,
      cover: coverPath,
      comicType: ComicType.local,
      downloadedChapters: chapters,
      createdAt: createTime ?? DateTime.now(),
    );
  }

  static Future<Map<String, String>> _copyDirectories(
    Map<String, dynamic> data,
  ) async {
    return overrideIO(() async {
      var toBeCopied = data['toBeCopied'] as List<String>;
      var destination = data['destination'] as String;
      Map<String, String> result = {};
      for (var dir in toBeCopied) {
        var source = Directory(dir);
        var dest = Directory("$destination/${source.name}");
        if (dest.existsSync()) {
          // The destination directory already exists, and it is not managed by the app.
          // Rename the old directory to avoid conflicts.
          Log.info(
            "Import Comic",
            "Directory already exists: ${source.name}\nRenaming the old directory.",
          );
          dest.renameSync(
            findValidDirectoryName(dest.parent.path, "${dest.path}_old"),
          );
        }
        dest.createSync();
        await copyDirectory(source, dest);
        result[source.path] = dest.path;
      }
      return result;
    });
  }

  Future<Map<String?, List<LocalComic>>> _copyComicsToLocalDir(
    Map<String?, List<LocalComic>> comics,
  ) async {
    var destPath = LocalManager().path;
    Map<String?, List<LocalComic>> result = {};
    for (var favoriteFolder in comics.keys) {
      result[favoriteFolder] = comics[favoriteFolder]!
          .where((c) => c.directory.startsWith(destPath))
          .toList();
      comics[favoriteFolder]!.removeWhere(
        (c) => c.directory.startsWith(destPath),
      );

      if (comics[favoriteFolder]!.isEmpty) {
        continue;
      }

      try {
        // copy the comics to the local directory
        var pathMap = await compute<Map<String, dynamic>, Map<String, String>>(
          _copyDirectories,
          {
            'toBeCopied': comics[favoriteFolder]!
                .map((e) => e.directory)
                .toList(),
            'destination': destPath,
          },
        );
        //Construct a new object since LocalComic.directory is a final String
        for (var c in comics[favoriteFolder]!) {
          result[favoriteFolder]!.add(
            LocalComic(
              id: c.id,
              title: c.title,
              subtitle: c.subtitle,
              tags: c.tags,
              directory: pathMap[c.directory]!,
              chapters: c.chapters,
              cover: c.cover,
              comicType: c.comicType,
              downloadedChapters: c.downloadedChapters,
              createdAt: c.createdAt,
            ),
          );
        }
      } catch (e, s) {
        App.rootContext.showMessage(message: "Failed to copy comics".tl);
        Log.error("Import Comic", e.toString(), s);
        return result;
      }
    }
    return result;
  }

  Future<bool> registerComics(
    Map<String?, List<LocalComic>> importedComics,
    bool copy,
  ) async {
    try {
      if (copy) {
        importedComics = await _copyComicsToLocalDir(importedComics);
      }
      int importedCount = 0;
      for (var folder in importedComics.keys) {
        for (var comic in importedComics[folder]!) {
          var id = LocalManager().findValidId(ComicType.local);
          LocalManager().add(comic, id);
          importedCount++;
          if (folder != null) {
            LocalFavoritesManager().addComic(
              folder,
              FavoriteItem(
                id: id,
                name: comic.title,
                coverPath: comic.cover,
                author: comic.subtitle,
                type: comic.comicType,
                tags: comic.tags,
                favoriteTime: comic.createdAt,
              ),
            );
          }
        }
      }
      App.rootContext.showMessage(
        message: "Imported @a comics".tlParams({'a': importedCount}),
      );
    } catch (e, s) {
      App.rootContext.showMessage(message: "Failed to register comics".tl);
      Log.error("Import Comic", e.toString(), s);
      return false;
    }
    return true;
  }
}
