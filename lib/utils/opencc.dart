import 'package:venera/src/rust/api/text.dart' as backend;

abstract class OpenCC {
  // Rust embeds the mapping table, so no asset loading is needed here.
  static Future<void> init() async {}

  static bool hasChineseSimplified(String text) =>
      backend.hasChineseSimplified(text: text);

  static bool hasChineseTraditional(String text) =>
      backend.hasChineseTraditional(text: text);

  static String simplifiedToTraditional(String text) =>
      backend.simplifiedToTraditional(text: text);

  static String traditionalToSimplified(String text) =>
      backend.traditionalToSimplified(text: text);
}
