import 'dart:convert';
import 'package:flutter_saf/flutter_saf.dart';
import 'package:venera/foundation/app.dart';
import 'package:venera/foundation/local.dart';
import 'package:venera/src/rust/api/media.dart' as backend;
import 'package:venera/utils/io.dart';

Future<File> exportLocalBook(
  LocalComic comic,
  String output,
  String format,
) async {
  final chapters = <(String, List<File>)>[];
  for (final id in comic.chapters == null ? ['0'] : comic.downloadedChapters) {
    final images = await LocalManager().getImages(
      comic.id,
      comic.comicType,
      id,
    );
    chapters.add((
      comic.chapters?[id] ?? comic.title,
      images.map((p) => File(p.replaceFirst(RegExp(r'^file://'), ''))).toList(),
    ));
  }
  return exportBookFiles(
    title: comic.title,
    author: comic.subtitle,
    cover: comic.coverFile,
    chapters: chapters,
    output: output,
    format: format,
    tags: comic.tags,
    hasChapters: comic.chapters != null,
  );
}

/// Android document-provider access stays in the platform adapter. Native paths
/// pass straight through; document-provider files are staged for the Rust worker.
Future<File> exportBookFiles({
  required String title,
  required String author,
  required File cover,
  required List<(String, List<File>)> chapters,
  required String output,
  required String format,
  List<String> tags = const [],
  bool hasChapters = false,
}) async {
  final staging = await Directory(App.cachePath).createTemp('book_');
  var index = 0;
  Future<String> nativePath(File file) async {
    if (file is! AndroidFile) return file.path;
    final target = staging.joinFile('${index++}.${file.extension}');
    await target.writeAsBytes(await file.readAsBytes());
    return target.path;
  }

  try {
    final data = {
      'title': title,
      'author': author,
      'tags': tags,
      'has_chapters': hasChapters,
      'cover': await nativePath(cover),
      'chapters': [
        for (final chapter in chapters)
          {
            'title': chapter.$1,
            'images': [for (final file in chapter.$2) await nativePath(file)],
          },
      ],
    };
    final target = File(output);
    final nativeOutput = target is AndroidFile
        ? staging.joinFile('output.$format').path
        : output;
    await backend.exportBook(
      bookJson: jsonEncode(data),
      output: nativeOutput,
      format: format,
    );
    if (target is AndroidFile) {
      await target.writeAsBytes(await File(nativeOutput).readAsBytes());
    }
    return target;
  } finally {
    await staging.delete(recursive: true);
  }
}
