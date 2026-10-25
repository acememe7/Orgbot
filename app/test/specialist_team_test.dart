import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/room_file.dart';

const _migrationKey = 'specialist_coordination_migration_version';
const _routingStart = '<!-- orgbot-specialist-team-routing:start -->';
const _routingEnd = '<!-- orgbot-specialist-team-routing:end -->';
const _legacyAstraPrefix =
    'You are the security and code reviewer. Review security and code changes; inspect evidence, '
    'identify concrete risks, and record actionable findings. Do not implement or review unrelated '
    'design work. Review Opus-authored code/security work independently only when that boundary is '
    'material. Avoid gratuitous calls and automatic cross-review cycles.';

Future<Map<String, Object?>> _preset() async =>
    (jsonDecode(await rootBundle.loadString('tool/specialist-team.json'))
            as Map)
        .cast<String, Object?>();

Map<String, Object?> _bot(Map<String, Object?> target) => {
  ...target,
  'additional_roles': [...(target['additional_roles'] as List)],
  'powers': Map<String, Object?>.from(target['powers'] as Map),
};

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'Linux migration updates only managed coordination fields and preserves data',
    () async {
      final preset = await _preset();
      final presetBots = preset['bots'] as List;
      final astra = _bot((presetBots[0] as Map).cast<String, Object?>())
        ..['additional_roles'] = ['reviewer', 'owner_custom']
        ..['powers'] = {
          'may_create_tasks': false,
          'may_inspect': false,
          'may_submit': false,
          'review_only': true,
          'owner_power': true,
        }
        ..['persona'] =
            '$_legacyAstraPrefix\n\nOwner personality and saved instructions.';
      final opus = _bot((presetBots[1] as Map).cast<String, Object?>())
        ..['additional_roles'] = ['designer', 'product_manager', 'owner_custom']
        ..['powers'] = {
          'may_create_tasks': false,
          'may_inspect': false,
          'may_submit': false,
          'review_only': true,
          'owner_power': 'keep',
        };
      final miles = _bot((presetBots[2] as Map).cast<String, Object?>())
        ..['role'] = 'coordinator'
        ..['additional_roles'] = ['support', 'owner_custom', 'coordinator']
        ..['powers'] = {
          'may_create_tasks': true,
          'may_inspect': true,
          'may_submit': true,
          'review_only': false,
          'owner_power': true,
        };
      final unrelated = {
        'id': 'owner_bot',
        'name': 'Custom',
        'handle': 'custom',
        'persona': 'Keep this person intact.',
        'provider': 'scripted',
        'model': 'fixture',
        'apiKey': 'fixture-key-marker',
        'role': 'writer',
        'additional_roles': ['owner_role'],
        'powers': {'may_inspect': true, 'owner_power': 42},
        'owner_field': {'retain': true},
      };
      final history = [
        {'id': 'event-1', 'body': 'saved transcript record'},
      ];
      final roomWithManagedMemory = {
        'id': 'room_a',
        'name': 'Existing room',
        'memory':
            'Owner note before\n\n$_routingStart\nstale routing\n$_routingEnd\n\nOwner note after',
        'bots': [
          {'bot_id': 'bot_0000000000000001'},
        ],
        'tasks': ['owner_task'],
        'history': history,
        'workspace': {'root': '/owner/project', 'writable': true},
        'owner_room_field': 'preserve',
      };
      final roomWithoutManagedMemory = {
        'id': 'room_b',
        'name': 'Another room',
        'memory': 'Owner-only note without migration markers.',
        'bots': [],
        'history': history,
      };
      final original = {
        'user': {'name': 'Alex', 'profile': 'Owner profile', 'owner_field': 7},
        'bots': [astra, opus, miles, unrelated],
        'rooms': [roomWithManagedMemory, roomWithoutManagedMemory],
        'owner_library_field': ['preserve'],
      };
      final saver = MemorySaver()..data = jsonEncode(original);
      final library = Library(saver: saver, isLinux: true);

      await library.load();

      expect(library.loaded, isTrue);
      expect(library.userName, 'Alex');
      expect(library.userProfile, 'Owner profile');
      final saved = jsonDecode(saver.data!) as Map<String, Object?>;
      expect(saved[_migrationKey], 1);
      expect(saved['owner_library_field'], ['preserve']);
      expect(saved['user'], original['user']);
      final savedBots = (saved['bots'] as List).cast<Map>();
      final savedAstra = savedBots.singleWhere(
        (bot) => bot['handle'] == 'astra',
      );
      final savedOpus = savedBots.singleWhere((bot) => bot['handle'] == 'opus');
      final savedMiles = savedBots.singleWhere(
        (bot) => bot['handle'] == 'grok',
      );
      final savedCustom = savedBots.singleWhere(
        (bot) => bot['handle'] == 'custom',
      );

      expect(savedAstra['id'], astra['id']);
      expect(savedAstra['apiKey'], astra['apiKey']);
      expect(savedAstra['role'], astra['role']);
      expect(savedAstra['additional_roles'], [
        'reviewer',
        'owner_custom',
        'coordinator',
      ]);
      expect(savedAstra['powers'], {
        'may_create_tasks': true,
        'may_inspect': false,
        'may_submit': true,
        'review_only': true,
        'owner_power': true,
      });
      final astraPersona = savedAstra['persona'] as String;
      expect(
        astraPersona,
        startsWith(
          (presetBots[0] as Map)['persona'].toString().split('\n\n').first,
        ),
      );
      expect(
        astraPersona,
        endsWith('Owner personality and saved instructions.'),
      );

      expect(savedOpus['additional_roles'], [
        'designer',
        'product_manager',
        'owner_custom',
        'coordinator',
      ]);
      expect(savedOpus['powers'], {
        'may_create_tasks': true,
        'may_inspect': false,
        'may_submit': true,
        'review_only': true,
        'owner_power': 'keep',
      });
      expect(savedOpus['apiKey'], opus['apiKey']);

      expect(savedMiles['role'], 'support');
      expect(savedMiles['additional_roles'], ['support', 'owner_custom']);
      expect(savedMiles['powers'], {
        'may_create_tasks': false,
        'may_inspect': true,
        'may_submit': false,
        'review_only': false,
        'owner_power': true,
      });
      expect(savedMiles['apiKey'], miles['apiKey']);
      expect(savedCustom, unrelated);

      final savedRooms = (saved['rooms'] as List).cast<Map>();
      expect(savedRooms[0]['bots'], roomWithManagedMemory['bots']);
      expect(savedRooms[0]['tasks'], roomWithManagedMemory['tasks']);
      expect(savedRooms[0]['history'], history);
      expect(savedRooms[0]['workspace'], roomWithManagedMemory['workspace']);
      expect(savedRooms[0]['owner_room_field'], 'preserve');
      final updatedMemory = savedRooms[0]['memory'] as String;
      expect(updatedMemory, startsWith('Owner note before'));
      expect(updatedMemory, endsWith('Owner note after'));
      expect(updatedMemory, contains(preset['routing_memory']));
      expect(updatedMemory, isNot(contains('stale routing')));
      expect(savedRooms[1]['memory'], roomWithoutManagedMemory['memory']);

      final emitted = buildRoomFile(
        RoomDef(
          name: 'Coordination check',
          bots: library.bots.map((bot) => RoomSlot(bot.id)).toList(),
        ),
        library.bots,
        canRunClis: true,
      ).toml;
      String blockFor(String handle) => emitted
          .split('[[bots]]')
          .singleWhere((block) => block.contains('handle = "$handle"'));
      final astraRoomBot = blockFor('astra');
      expect(astraRoomBot, contains('"coordinator"'));
      expect(astraRoomBot, contains('may_create_tasks = true'));
      expect(astraRoomBot, contains('may_submit = true'));
      expect(astraRoomBot, contains('review_only = true'));
      final opusRoomBot = blockFor('opus');
      expect(opusRoomBot, contains('"coordinator"'));
      expect(opusRoomBot, contains('may_create_tasks = true'));
      final milesRoomBot = blockFor('grok');
      expect(milesRoomBot, contains('"support"'));
      expect(milesRoomBot, isNot(contains('"coordinator"')));
      expect(milesRoomBot, contains('may_create_tasks = false'));
    },
  );

  test('migration marker keeps later owner edits on reload', () async {
    final preset = await _preset();
    final bots = (preset['bots'] as List)
        .take(3)
        .map((bot) => _bot((bot as Map).cast<String, Object?>()))
        .toList();
    final saver = MemorySaver()..data = jsonEncode({'bots': bots, 'rooms': []});
    final first = Library(saver: saver, isLinux: true);
    await first.load();
    final astra = first.bots.singleWhere((bot) => bot.handle == 'astra');
    astra.additionalRoles.remove('coordinator');
    await first.save();

    final second = Library(saver: saver, isLinux: true);
    await second.load();

    expect(
      second.bots.singleWhere((bot) => bot.handle == 'astra').roles,
      isNot(contains('coordinator')),
    );
    expect((jsonDecode(saver.data!) as Map)[_migrationKey], 1);
  });

  test(
    'custom identity mismatches stay unchanged and migration completes once',
    () async {
      final preset = await _preset();
      final targetAstra = (preset['bots'] as List).first as Map;
      final customAstra = _bot(targetAstra.cast<String, Object?>())
        ..['model'] = 'owner-selected-model'
        ..['additional_roles'] = ['owner_custom']
        ..['powers'] = {'may_create_tasks': false, 'owner_power': true};
      final saver = MemorySaver()
        ..data = jsonEncode({
          'bots': [customAstra],
          'rooms': [],
        });
      final library = Library(saver: saver, isLinux: true);

      await library.load();

      final stored = jsonDecode(saver.data!) as Map<String, Object?>;
      final savedBot = (stored['bots'] as List).single as Map;
      expect(savedBot['model'], 'owner-selected-model');
      expect(savedBot['additional_roles'], ['owner_custom']);
      expect(savedBot['powers'], customAstra['powers']);
      expect(stored[_migrationKey], 1);
    },
  );

  test('Android load does not apply Linux coordination migration', () async {
    final preset = await _preset();
    final astra = _bot(
      ((preset['bots'] as List).first as Map).cast<String, Object?>(),
    );
    astra['additional_roles'] = ['reviewer'];
    final saver = MemorySaver()
      ..data = jsonEncode({
        'bots': [astra],
        'rooms': [],
      });
    final library = Library(saver: saver, isLinux: false);

    await library.load();

    expect(library.bots.single.roles, ['security', 'reviewer']);
    final stored = jsonDecode(saver.data!) as Map<String, Object?>;
    expect(stored.containsKey(_migrationKey), isFalse);
    expect((stored['bots'] as List).single['additional_roles'], ['reviewer']);
  });

  test('failed secure save keeps original data and load can retry', () async {
    final preset = await _preset();
    final bots = (preset['bots'] as List)
        .take(3)
        .map((bot) => _bot((bot as Map).cast<String, Object?>()))
        .toList();
    final original = jsonEncode({'bots': bots, 'rooms': []});
    final saver = _FailingMigrationSaver(original);
    final library = Library(saver: saver, isLinux: true);

    await library.load();

    expect(library.loaded, isFalse);
    expect(library.bots, isEmpty);
    expect(library.error, contains('could not be securely updated'));
    expect(saver.data, original);

    await library.load();

    expect(library.loaded, isTrue);
    expect(library.error, isNull);
    expect((jsonDecode(saver.data!) as Map)[_migrationKey], 1);
  });

  test('malformed original bot data is not saved during migration', () async {
    final original = jsonEncode({
      'user': {'name': 'Alex'},
      'bots': [
        {'name': 'Missing required identity'},
      ],
      'rooms': [],
    });
    final saver = _CountingSaver(original);
    final library = Library(saver: saver, isLinux: true);

    await library.load();

    expect(library.loaded, isFalse);
    expect(library.bots, isEmpty);
    expect(library.error, contains('kept intact'));
    expect(saver.data, original);
    expect(saver.saveCount, 0);
  });

  test(
    'legacy library moves to secure storage while preserving fields',
    () async {
      final legacy = jsonEncode({
        'user': {'name': 'Alex', 'profile': 'Existing profile'},
        'bots': [BotDef(name: 'D', apiKey: 'private-key').toJson()],
        'rooms': [RoomDef(name: 'Existing room').toJson()],
      });
      SharedPreferences.setMockInitialValues({'orgbot_library_v1': legacy});
      FlutterSecureStorage.setMockInitialValues({});
      final library = Library(isLinux: false);

      await library.load();

      expect(library.bots.single.apiKey, 'private-key');
      expect(
        (await SharedPreferences.getInstance()).getString('orgbot_library_v1'),
        isNull,
      );
      final secured =
          jsonDecode(
                (await const FlutterSecureStorage().read(
                  key: 'orgbot_library_v1',
                ))!,
              )
              as Map<String, dynamic>;
      final original = jsonDecode(legacy) as Map<String, dynamic>;
      expect(secured, original);
    },
  );
}

class _CountingSaver extends MemorySaver {
  _CountingSaver(String initial) {
    data = initial;
  }

  int saveCount = 0;

  @override
  Future<void> save(String json) async {
    saveCount++;
    data = json;
  }
}

class _FailingMigrationSaver extends MemorySaver {
  _FailingMigrationSaver(String initial) {
    data = initial;
  }

  bool _failNextSave = true;

  @override
  Future<void> save(String json) async {
    if (_failNextSave) {
      _failNextSave = false;
      throw StateError('fixture secure write failure');
    }
    data = json;
  }
}
