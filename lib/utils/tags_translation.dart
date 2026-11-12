// Dictionary source: https://github.com/EhTagTranslation/Database/tree/master/database
// Traditional Chinese contributed by https://github.com/NeKoOuO
// Tag dictionaries and matching are owned by the Rust backend.
import 'package:venera/foundation/app.dart';
import 'package:venera/foundation/global_state.dart';
import 'package:venera/src/rust/api/tags.dart' as backend;

extension TagsTranslation on String {
  static bool get _traditional => App.locale.countryCode == 'TW';
  static Future<void> readData() async {}
  String get translateTagsToCN =>
      backend.translateTag(text: this, traditional: _traditional);
  String get translateTagIfNeed =>
      App.locale.languageCode == 'zh' ? translateTagsToCN : this;
  static String translateTag(String tag) {
    if (tag.contains(':') && tag.indexOf(':') == tag.lastIndexOf(':')) {
      final parts = tag.split(':');
      return translationTagWithNamespace(parts[1], parts[0]);
    }
    return tag.translateTagsToCN;
  }

  static String translationTagWithNamespace(String text, String namespace) =>
      backend.translateTag(
        text: text,
        namespace: namespace,
        traditional: _traditional,
      );
  static List<Pair<String, TranslationType>> suggestions(
    String text,
    int limit,
  ) => backend
      .tagSuggestions(text: text, traditional: _traditional, limit: limit)
      .map((v) => Pair(v.text, TranslationType.values.byName(v.namespace)))
      .toList();
  String get categoryTextDynamic =>
      App.locale.languageCode == 'zh' ? translateTagsCategoryToCN : this;
  String get translateTagsCategoryToCN =>
      tagsCategoryTranslations[this] ?? this;
  Map<String, String> get tagsCategoryTranslations =>
      _traditional ? tagsCategoryTranslationsTW : tagsCategoryTranslationsCN;

  static const tagsCategoryTranslationsCN = {
    "language": "语言",
    "artist": "画师",
    "male": "男性",
    "female": "女性",
    "mixed": "混合",
    "other": "其它",
    "parody": "原作",
    "character": "角色",
    "group": "团队",
    "cosplayer": "Coser",
    "reclass": "重新分类",
    "Languages": "语言",
    "Artists": "画师",
    "Characters": "角色",
    "Groups": "团队",
    "Tags": "标签",
    "Parodies": "原作",
    "Categories": "分类",
    "Time": "时间",
  };

  static const tagsCategoryTranslationsTW = {
    "language": "語言",
    "artist": "畫師",
    "male": "男性",
    "female": "女性",
    "mixed": "混合",
    "other": "其他",
    "parody": "原作",
    "character": "角色",
    "group": "團隊",
    "cosplayer": "Coser",
    "reclass": "重新分類",
    "Languages": "語言",
    "Artists": "畫師",
    "Characters": "角色",
    "Groups": "團隊",
    "Tags": "標籤",
    "Parodies": "原作",
    "Categories": "分類",
    "Time": "時間",
  };
}

enum TranslationType {
  female,
  male,
  mixed,
  language,
  other,
  group,
  artist,
  cosplayer,
  parody,
  character,
  reclass,
}
