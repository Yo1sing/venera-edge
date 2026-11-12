import 'dart:convert';

import 'package:venera/foundation/app.dart';
import 'package:venera/foundation/appdata.dart';
import 'package:venera/foundation/comic_source/comic_source.dart';
import 'package:venera/foundation/favorites.dart';
import 'package:venera/foundation/history.dart';
import 'package:venera/network/cookie_jar.dart';
import 'package:venera/src/rust/api/backup.dart' as backend;

import 'io.dart';

Future<File> exportAppData([bool sync = true]) async => File(
  await backend.exportAppData(
    directory: App.dataPath,
    cache: App.cachePath,
    sync_: sync,
  ),
);

Future<void> importAppData(File file, [bool checkVersion = false]) async {
  final prepared = await backend.PreparedBackup.open(
    input: file.path,
    cache: App.cachePath,
    currentVersion: checkVersion ? appdata.settings['dataVersion'] : null,
  );
  if (prepared == null) return;
  try {
    HistoryManager().close();
    LocalFavoritesManager().close();
    SingleInstanceCookieJar.instance?.dispose();
    try {
      final result = jsonDecode(await prepared.apply(directory: App.dataPath));
      appdata.applyBackendData(Map<String, dynamic>.from(result['data']));
      if (result['sourcesChanged'] == true) await ComicSourceManager().reload();
    } finally {
      await HistoryManager().init();
      await LocalFavoritesManager().init();
      SingleInstanceCookieJar.instance = SingleInstanceCookieJar(
        '${App.dataPath}/cookie.db',
      )..init();
    }
  } finally {
    prepared.dispose();
  }
}

Future<void> importPicaData(File file) async {
  await backend.importPicaData(
    input: file.path,
    cache: App.cachePath,
    directory: App.dataPath,
    sourceTypesJson: jsonEncode({
      for (final key in [
        'picacg',
        'ehentai',
        'jm',
        'hitomi',
        'wnacg',
        'nhentai',
      ])
        key: key.hashCode,
    }),
    installed: ComicSource.all().map((s) => s.key).toList(),
    traditional: App.locale.countryCode == 'TW',
    atEnd: appdata.settings['newFavoriteAddTo'] == 'end',
  );
  LocalFavoritesManager().initCounts();
  ImageFavoriteManager().init();
}
