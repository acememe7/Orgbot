import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/lend_settings.dart';
import 'package:orgbot_app/state/engine_room_controller.dart';

void main() {
  late Directory home;

  setUp(() => home = Directory.systemTemp.createTempSync('orgbot-lend-test-'));
  tearDown(() => home.deleteSync(recursive: true));

  Map<String, String> env() => {'HOME': home.path};

  test('every known tool starts off and your choices are kept', () {
    final first = LendSettings.load(environment: env());
    expect(first.lent, isEmpty, reason: 'nothing is lent until you turn it on');
    final wrangler = first.entries.firstWhere(
      (entry) => entry.name == 'wrangler',
    );
    expect(first.isPresent(wrangler), isFalse);
    wrangler
      ..enabled = true
      ..risk = LendRisk.regular;
    first.entries.add(
      LendEntry(
        name: 'mytool',
        label: 'My tool',
        paths: ['.config/mytool'],
        enabled: true,
      ),
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
    final again = second.entries.firstWhere(
      (entry) => entry.name == 'wrangler',
    );
    expect(again.risk, LendRisk.regular);
    expect(second.isPresent(again), isTrue);
  });

  test('keys, OrgBots data and paths outside your home are never lent', () {
    for (final path in [
      '.ssh',
      '.ssh/id_ed25519',
      '.local/share/orgbot',
      '.codex/auth.json',
      '../x',
      '/etc',
      '',
    ]) {
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

  test('every switched-off sign-in is hidden, yours included', () {
    final settings = LendSettings.load(environment: env());
    settings.entries.firstWhere((entry) => entry.name == 'gh').enabled = true;
    settings.entries.add(
      LendEntry(name: 'mytool', label: 'My tool', paths: ['.config/mytool']),
    );
    expect(
      settings.hidden,
      contains('.config/mytool'),
      reason:
          'from the live room: a switched-off custom sign-in was not hidden',
    );
    expect(settings.hidden, contains('.docker/config.json'));
    expect(
      settings.hidden,
      isNot(contains('.config/gh')),
      reason: 'a lent one is hidden by the room as a lend instead',
    );
  });

  test('sign-ins and tools on this computer are found', () {
    Directory('${home.path}/.config/gh').createSync(recursive: true);
    final bin = Directory('${home.path}/bin')..createSync();
    final docker = File('${bin.path}/docker')..writeAsStringSync('#!/bin/sh\n');
    Process.runSync('chmod', ['755', docker.path]);
    final settings = LendSettings.load(
      environment: {...env(), 'PATH': '${bin.path}:/nonexistent'},
    );
    expect(settings.found.map((entry) => entry.name), ['gh']);
    expect(settings.installedNotSignedIn.map((entry) => entry.name), [
      'docker',
    ]);
    settings.entries.firstWhere((entry) => entry.name == 'gh').enabled = true;
    expect(
      settings.found,
      isEmpty,
      reason: 'a sign-in already turned on is not suggested',
    );
  });

  test('a sign-in a bot asked for and you allowed is kept, turned on', () {
    final settings = LendSettings.load(environment: env());
    expect(
      settings.addApproved(
        LendEntry(name: 'keys', label: 'Keys', paths: ['.ssh']),
      ),
      isFalse,
      reason: 'never lent, even when a room allowed it',
    );
    expect(
      settings.addApproved(
        LendEntry(
          name: 'capture',
          label: 'Capture registry',
          paths: ['.config/capture'],
        ),
      ),
      isTrue,
    );
    expect(
      settings.addApproved(
        LendEntry(
          name: 'docker',
          label: 'Docker Hub',
          paths: ['.docker/config.json'],
        ),
      ),
      isTrue,
    );
    settings.save();
    final again = LendSettings.load(environment: env());
    expect(
      again.lent.map((entry) => entry.name),
      containsAll(['capture', 'docker']),
    );
  });

  test(
    'the room telling the app about an allowed sign-in turns it on in your list',
    () {
      keepApprovedSignIn({
        'name': 'capture',
        'label': 'Capture registry',
        'paths': ['.config/capture'],
        'risk': 'critical',
      }, load: () => LendSettings.load(environment: env()));
      final kept = LendSettings.load(environment: env()).lent.single;
      expect(kept.name, 'capture');
      expect(kept.risk, LendRisk.critical);
    },
  );
}
