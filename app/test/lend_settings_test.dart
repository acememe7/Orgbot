import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/lend_settings.dart';

void main() {
  late Directory home;

  setUp(() => home = Directory.systemTemp.createTempSync('orgbot-lend-test-'));
  tearDown(() => home.deleteSync(recursive: true));

  Map<String, String> env() => {'HOME': home.path};

  test('every known tool starts off and your choices are kept', () {
    final first = LendSettings.load(environment: env());
    expect(first.lent, isEmpty, reason: 'nothing is lent until you turn it on');
    final wrangler = first.entries.firstWhere((entry) => entry.name == 'wrangler');
    expect(first.isPresent(wrangler), isFalse);
    wrangler
      ..enabled = true
      ..risk = LendRisk.regular;
    first.entries.add(
      LendEntry(name: 'mytool', label: 'My tool', paths: ['.config/mytool'], enabled: true),
    );
    first.save();
    expect(
      File('${home.path}/.local/share/orgbot/lending.json').existsSync(),
      isTrue,
      reason: 'kept in the data folder bots cannot read',
    );

    Directory('${home.path}/.config/.wrangler').createSync(recursive: true);
    final second = LendSettings.load(environment: env());
    expect(second.lent.map((entry) => entry.name), ['wrangler', 'mytool']);
    final again = second.entries.firstWhere((entry) => entry.name == 'wrangler');
    expect(again.risk, LendRisk.regular);
    expect(second.isPresent(again), isTrue);
  });

  test('keys, OrgBots data and paths outside your home are never lent', () {
    for (final path in ['.ssh', '.ssh/id_ed25519', '.local/share/orgbot', '.codex/auth.json', '../x', '/etc', '']) {
      expect(lendPathProblem(path), isNotNull, reason: path);
    }
    expect(lendPathProblem('.config/.wrangler'), isNull);
  });

  test('a hand-edited list cannot sneak in a forbidden path', () {
    final file = File('${home.path}/.local/share/orgbot/lending.json')
      ..createSync(recursive: true)
      ..writeAsStringSync(
        '{"lend":[{"name":"keys","label":"Keys","paths":[".ssh"],"enabled":true}]}',
      );
    expect(file.existsSync(), isTrue);
    expect(LendSettings.load(environment: env()).lent, isEmpty);
  });
}
