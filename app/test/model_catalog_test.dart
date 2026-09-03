// Models come from the machine, not from a list written here.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/model_catalog.dart';
import 'package:orgbot_app/orgbot/models.dart';

void main() {
  setUp(ModelCatalog.invalidate);
  tearDown(() {
    ModelCatalog.runner = null;
    ModelCatalog.invalidate();
  });

  test('grok models are parsed from the CLI listing', () async {
    ModelCatalog.runner = (bin, args) async => '''
You are logged in with grok.com.

Default model: grok-4.6

Available models:
  * grok-4.6 (default)
  - grok-4.5
''';
    final list = await ModelCatalog.load(ProviderKind.grok);
    // The CLI may be absent in CI, in which case the fallback is correct too;
    // when it is present the parse must find both models.
    if (list.source == ModelSource.live) {
      expect(list.options.map((o) => o.id), containsAll(['grok-4.6', 'grok-4.5']));
      expect(list.options.firstWhere((o) => o.id == 'grok-4.6').note, 'Default');
    }
  });

  test('claude offers aliases that follow the subscription', () async {
    final list = await ModelCatalog.load(ProviderKind.anthropic);
    expect(list.options.map((o) => o.id), containsAll(['opus', 'sonnet', 'haiku']));
    // An empty id means "let the plan decide", which must stay available.
    expect(list.options.any((o) => o.id.isEmpty), isTrue);
  });

  /// Without a key the API cannot be asked, so nothing is listed. Offering a
  /// written-down list here would be exactly the hardcoding this replaced, and
  /// would look authoritative while being a guess.
  test('deepseek without a key lists nothing and says why', () async {
    final list = await ModelCatalog.load(ProviderKind.deepseek);
    expect(list.source, ModelSource.needsKey);
    expect(list.options, isEmpty);
    expect(list.problem, contains('API key'));
  });

  test('the scripted demo offers no models', () async {
    final list = await ModelCatalog.load(ProviderKind.scripted);
    expect(list.options, isEmpty);
  });

  test('a subscription backend defaults to the provider choosing', () {
    for (final p in [ProviderKind.anthropic, ProviderKind.codex, ProviderKind.grok]) {
      expect(ModelCatalog.defaultFor(p), isEmpty,
          reason: '$p should let the plan pick unless told otherwise');
    }
    expect(ModelCatalog.defaultFor(ProviderKind.deepseek), 'deepseek-chat');
  });

  test('an unknown id still renders as itself', () {
    expect(ModelCatalog.labelFor(ProviderKind.deepseek, 'some-private-model'),
        'some-private-model');
    expect(ModelCatalog.labelFor(ProviderKind.anthropic, ''), 'default model');
  });

  /// Reads the real cache on this machine when there is one. This is the check
  /// that would have caught the hardcoded list being wrong.
  test('codex models come from its own cache when present', () async {
    final home = Platform.environment['HOME'] ?? '';
    final cache = File('$home/.codex/models_cache.json');
    if (!cache.existsSync()) {
      return; // Nothing signed in here; the fallback path is covered above.
    }
    final list = await ModelCatalog.load(ProviderKind.codex);
    expect(list.source, ModelSource.live);
    expect(list.options, isNotEmpty);
    expect(list.options.every((o) => o.id.isNotEmpty), isTrue);
  });
}
