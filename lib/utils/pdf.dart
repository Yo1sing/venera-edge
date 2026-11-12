import 'package:venera/foundation/local.dart';
import 'package:venera/utils/book_export.dart';
import 'package:venera/utils/io.dart';

Future<File> createPdfFromComicIsolate(LocalComic comic, String savePath) =>
    exportLocalBook(comic, savePath, 'pdf');
