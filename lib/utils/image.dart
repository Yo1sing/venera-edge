import 'dart:typed_data';
import 'package:venera/src/rust/api/media.dart' as backend;

Future<Uint8List> modifyImageWithScript(Uint8List data, String script) =>
    backend.modifyImage(data: data, script: script);
