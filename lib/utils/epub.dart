import 'package:venera/foundation/local.dart';
import 'package:venera/utils/book_export.dart';
import 'package:venera/utils/io.dart';

class EpubData {
  final String title;
  final String author;
  final File cover;
  final Map<String, List<File>> chapters;
  const EpubData({
    required this.title,
    required this.author,
    required this.cover,
    required this.chapters,
  });
}

Future<File> createEpubComic(
  EpubData data,
  String cacheDir,
  String outFilePath,
) => exportBookFiles(
  title: data.title,
  author: data.author,
  cover: data.cover,
  chapters: data.chapters.entries.map((e) => (e.key, e.value)).toList(),
  output: outFilePath,
  format: 'epub',
);

Future<File> createEpubWithLocalComic(LocalComic comic, String outFilePath) =>
    exportLocalBook(comic, outFilePath, 'epub');
