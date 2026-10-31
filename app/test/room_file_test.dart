// The room file the app generates has to be one the engine will actually run.
// A file that only looks right fails at `serve` time with a parse error, so
// these assert the shape the Rust config expects.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/orgbot/lend_settings.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/model_catalog.dart';
import 'package:orgbot_app/orgbot/room_file.dart';
import 'package:orgbot_app/orgbot/roles.dart';
import 'package:shared_preferences/shared_preferences.dart';

RoomDef room({List<String> tasks = const []}) => RoomDef(
  name: 'Release notes',
  goal: 'Write the 1.3 notes',
  path: '/tmp/project',
  tasks: tasks,
);

void main() {
  _roleTableTests();

  test('review-only permissions survive edits and use a read-only CLI', () {
    final powers = RolePowers.fromJson({
      'review_only': true,
      'may_inspect': true,
    }).copyWith(maySubmit: false);
    expect(powers.toJson()['review_only'], isTrue);
    expect(RolePowers.fromJson({}).reviewOnly, isFalse);
    final bot = BotDef(
      name: 'Astra',
      provider: ProviderKind.codex,
      model: 'gpt-6-astra',
      role: 'security',
      powers: powers,
    );
    final restored = BotDef.fromJson(bot.toJson());
    final file = buildRoomFile(room(), [restored], canRunClis: true);
    expect(file.toml, contains('review_only = true'));
    expect(file.toml, contains('policy = "read_only"'));
    expect(file.toml, contains('max_turns_per_task = 8'));
    expect(file.toml, contains('max_open_handoffs = 4'));
    final worker = buildRoomFile(room(), [
      BotDef(name: 'Builder'),
    ], canRunClis: true);
    expect(worker.toml, contains('policy = "workspace_write"'));
    expect(worker.toml, contains('max_turns_per_task = 24'));
    final phone = buildRoomFile(room(), [
      BotDef(name: 'Demo', provider: ProviderKind.scripted),
    ], canRunClis: false);
    expect(phone.toml, isNot(contains('max_turns_per_task')));
  });

  // Concurrency is a rate multiplier, not only a speed-up: a room running four
  // at a time draws about four times as fast as one at a time. A default that
  // quadruples someone's consumption is a decision taken on their behalf.
  test('a new room does not default to quadrupling what it draws', () {
    expect(defaultParallelTurns, lessThanOrEqualTo(2));
    expect(
      defaultParallelTurns,
      greaterThan(1),
      reason: 'but still better than one bot at a time',
    );
  });

  test(
    'a room saved before the setting existed picks up the safer default',
    () {
      final restored = RoomDef.fromJson({
        'id': 'room_old',
        'name': 'Existing',
        'goal': 'carry on',
      });
      expect(restored.parallelTurns, defaultParallelTurns);
    },
  );

  // ---------------------------------------------------------- Claude on a phone
  //
  // A Claude sign-in reaches a phone through a pairing bundle, not a sign-in
  // flow: there is no device-code flow for Anthropic, so the desktop's own
  // credential is carried across and the engine calls the Messages API directly.
  // The list of models offered has to match the route taken, and it did not.

  test(
    'a device with no CLI is offered model ids, not Claude Code aliases',
    () {
      final onPhone = ModelCatalog.claudeModels(viaCli: false);
      expect(onPhone, isNotEmpty);
      for (final option in onPhone) {
        expect(
          option.id,
          startsWith('claude-'),
          reason: '`${option.id}` is an alias the Messages API will not accept',
        );
      }
    },
  );

  test('a computer with the CLI keeps the aliases, which follow the plan', () {
    final ids = ModelCatalog.claudeModels(
      viaCli: true,
    ).map((m) => m.id).toList();
    expect(ids, contains('opus'));
    expect(
      ids,
      contains(''),
      reason: 'and the option to let the subscription choose',
    );
  });

  // The engine runs one bot at a time unless the room file says otherwise,
  // which is how a room could hold twenty bots and have nineteen idle.
  test('the room file says how many bots may work at once', () {
    final bot = BotDef(name: 'Demo', provider: ProviderKind.scripted);
    final toml = buildRoomFile(
      RoomDef(name: 'Busy', goal: 'get on with it', parallelTurns: 6),
      [bot],
      canRunClis: false,
    ).toml;
    expect(toml, contains('max_parallel_turns = 6'));
  });

  test('a room set to one bot at a time still says so explicitly', () {
    final bot = BotDef(name: 'Demo', provider: ProviderKind.scripted);
    final toml = buildRoomFile(
      RoomDef(name: 'Careful', goal: 'one thing at a time', parallelTurns: 1),
      [bot],
      canRunClis: false,
    ).toml;
    expect(toml, contains('max_parallel_turns = 1'));
  });

  test(
    'the room file carries the access mode and only the sign-ins you lend',
    () {
      final bot = BotDef(name: 'Demo', provider: ProviderKind.scripted);
      final lent = LendEntry(
        name: 'wrangler',
        label: 'Cloudflare (Wrangler)',
        paths: ['.config/.wrangler'],
        enabled: true,
      );
      final off = LendEntry(
        name: 'gh',
        label: 'GitHub CLI',
        paths: ['.config/gh'],
      );
      final toml = buildRoomFile(
        RoomDef(
          name: 'Release',
          goal: 'ship it',
          access: AccessMode.halfApproval,
        ),
        [bot],
        canRunClis: false,
        lend: [lent, off],
      ).toml;
      expect(toml, contains('access = "half_approval"'));
      expect(toml, contains('[[room.lend]]\nname = "wrangler"'));
      expect(toml, contains('paths = [".config/.wrangler"]'));
      expect(toml, contains('risk = "risky"'));
      expect(
        toml,
        isNot(contains('"gh"')),
        reason: 'a sign-in you did not turn on is not offered',
      );
    },
  );

  test(
    'switched-off sign-ins are written as hidden paths in the room table',
    () {
      final bot = BotDef(name: 'Demo', provider: ProviderKind.scripted);
      final toml = buildRoomFile(
        RoomDef(name: 'Release', goal: 'ship it'),
        [bot],
        canRunClis: false,
        hide: ['.config/gh', '.config/mytool'],
      ).toml;
      final room = toml.substring(
        toml.indexOf('[room]'),
        toml.indexOf('\n\n', toml.indexOf('access =')),
      );
      expect(room, contains('hide = [".config/gh", ".config/mytool"]'));
    },
  );

  test('a room saved before access modes existed asks for everything', () {
    final restored = RoomDef.fromJson({'id': 'room_old', 'name': 'Existing'});
    expect(restored.access, AccessMode.approval);
    final saved = RoomDef.fromJson(
      RoomDef(name: 'Busy', access: AccessMode.auto).toJson(),
    );
    expect(saved.access, AccessMode.auto);
  });

  test('a room saved before the setting existed gets the default', () {
    final restored = RoomDef.fromJson({
      'id': 'room_old',
      'name': 'Existing',
      'goal': 'carry on',
      'max_turns': 2000,
    });
    expect(restored.parallelTurns, defaultParallelTurns);
    expect(
      restored.parallelTurns,
      greaterThan(1),
      reason: 'an existing room should speed up without being rebuilt',
    );
  });
  test(
    'Android rooms have no hidden time cutoff; CLI defaults are independent',
    () {
      final bot = BotDef(name: 'Demo', provider: ProviderKind.scripted);
      final android = buildRoomFile(
        room(),
        [bot],
        canRunClis: false,
        statePath: '/private/state.json',
      ).toml;
      expect(android, contains('[room.limits]\nmax_duration_ms = 0'));
      expect(
        android.indexOf('state_path'),
        lessThan(android.indexOf('[room.limits]')),
      );
      expect(
        buildRoomFile(room(), [bot], canRunClis: true).toml,
        contains('max_duration_ms = 0'),
      );
    },
  );
  test('different DeepSeek bot keys stay separate in a room', () {
    final bots = [
      BotDef(name: 'one', provider: ProviderKind.deepseek, apiKey: 'key-one'),
      BotDef(name: 'two', provider: ProviderKind.deepseek, apiKey: 'key-two'),
    ];
    final file = buildRoomFile(room(), bots, canRunClis: false);
    expect(file.environment['DEEPSEEK_API_KEY'], 'key-one');
    expect(file.environment['ORGBOT_DEEPSEEK_TWO_API_KEY'], 'key-two');
    expect(file.toml, contains('profile = "deepseek_two"'));
    expect(file.toml, isNot(contains('key-one')));
    expect(file.toml, isNot(contains('key-two')));
  });
  test('a subscription bot becomes a cli_agent with MCP on', () {
    final bot = BotDef(
      name: 'Iris',
      handle: 'iris',
      provider: ProviderKind.anthropic,
      role: 'reviewer',
    );
    final file = buildRoomFile(room(), [bot]);

    expect(file.toml, contains('kind = "cli_agent"'));
    expect(file.toml, contains('cli = "claude"'));
    // MCP is the whole point: it gives the agent the board as tools alongside
    // its own memories and skills.
    expect(file.toml, contains('mcp = true'));
    expect(file.toml, contains('workdir = "/tmp/project"'));
    expect(file.skipped, isEmpty);
  });

  test('a chosen effort is written only where the CLI offers it', () {
    String toml(ProviderKind provider, String effort) => buildRoomFile(room(), [
      BotDef(name: 'b', handle: 'b', provider: provider, effort: effort),
    ], canRunClis: true).toml;

    expect(toml(ProviderKind.anthropic, 'max'), contains('effort = "max"'));
    expect(toml(ProviderKind.codex, 'xhigh'), contains('effort = "xhigh"'));
    expect(toml(ProviderKind.grok, 'low'), contains('effort = "low"'));
    if (Platform.isLinux) {
      expect(
        toml(ProviderKind.antigravity, 'high'),
        contains('effort = "high"'),
      );
      // OpenCode takes effort as a model variant, so nothing is written.
      expect(toml(ProviderKind.opencode, 'high'), isNot(contains('effort')));
    }
    // Codex has no `max`: left over from another CLI, it is dropped so the
    // room still opens with the CLI's default.
    expect(toml(ProviderKind.codex, 'max'), isNot(contains('effort')));
    expect(toml(ProviderKind.anthropic, ''), isNot(contains('effort')));
  });

  test('a bot keeps its effort across saves', () {
    final bot = BotDef(name: 'b', provider: ProviderKind.codex, effort: 'high');
    expect(BotDef.fromJson(bot.toJson()).effort, 'high');
    expect(BotDef.fromJson({'id': 'old', 'name': 'b'}).effort, '');
  });

  test('each CLI maps to its canonical runtime name without auth env', () {
    if (!Platform.isLinux) return;
    for (final pair in [
      (ProviderKind.anthropic, 'claude'),
      (ProviderKind.codex, 'codex'),
      (ProviderKind.grok, 'grok'),
      (ProviderKind.opencode, 'opencode'),
      (ProviderKind.antigravity, 'antigravity'),
    ]) {
      final file = buildRoomFile(room(), [
        BotDef(name: 'b', handle: 'b', provider: pair.$1),
      ], canRunClis: true);
      expect(file.toml, contains('cli = "${pair.$2}"'));
      expect(file.environment, isEmpty);
      expect(file.skipped, isEmpty);
    }
  });

  test('protected CLI providers are skipped outside the Linux runtime', () {
    final file = buildRoomFile(room(), [
      BotDef(name: 'OpenCode', handle: 'open', provider: ProviderKind.opencode),
      BotDef(
        name: 'Antigravity',
        handle: 'agy',
        provider: ProviderKind.antigravity,
      ),
    ], canRunClis: false);
    expect(file.toml, isNot(contains('kind = "cli_agent"')));
    expect(file.environment, isEmpty);
    expect(file.skipped, hasLength(2));
    expect(file.skipped.join(' '), contains('protected Linux runtime'));
  });

  test('desktop CLI rooms ignore saved phone OAuth credentials', () async {
    SharedPreferences.setMockInitialValues({});
    FlutterSecureStorage.setMockInitialValues({});
    final credentials = Credentials();
    addTearDown(credentials.dispose);
    const providers = [
      ProviderKind.codex,
      ProviderKind.anthropic,
      ProviderKind.grok,
    ];
    await credentials.importBundle(
      jsonEncode({
        'version': 1,
        'providers': [
          for (final provider in providers)
            {'provider': provider.key, 'token': 'phone-fixture-only'},
        ],
      }),
    );
    final file = buildRoomFile(
      room(),
      [
        for (final provider in providers)
          BotDef(name: provider.key, provider: provider),
      ],
      credentials: credentials,
      canRunClis: true,
    );
    expect(file.skipped, isEmpty);
    expect(file.environment, isEmpty);
    expect(RegExp('kind = "cli_agent"').allMatches(file.toml), hasLength(3));
    expect(RegExp('mcp = true').allMatches(file.toml), hasLength(3));
    expect(
      RegExp('workdir = "/tmp/project"').allMatches(file.toml),
      hasLength(3),
    );
    expect(file.toml, isNot(contains('phone-fixture-only')));
  });

  test('the role decides the permissions written into the file', () {
    final reviewer = buildRoomFile(room(), [
      BotDef(name: 'q', handle: 'q', role: 'reviewer'),
    ]).toml;
    expect(reviewer, contains('may_inspect = true'));
    expect(
      reviewer,
      contains('may_submit = false'),
      reason: 'an inspector must not also deliver',
    );

    final lead = buildRoomFile(room(), [
      BotDef(name: 'l', handle: 'l', role: 'coordinator'),
    ]).toml;
    expect(lead, contains('may_create_tasks = true'));
    expect(lead, contains('may_submit = true'));
    expect(
      lead,
      contains('may_inspect = false'),
      reason: 'a coordinator must not sign off on its own plan',
    );
  });

  test('an explicit power override beats the role default', () {
    final bot = BotDef(name: 'x', handle: 'x', role: 'engineer')
      ..powers = const RolePowers(mayInspect: true);
    expect(buildRoomFile(room(), [bot]).toml, contains('may_inspect = true'));
  });

  test('the role brief is folded into the persona', () {
    final bot = BotDef(
      name: 'x',
      handle: 'x',
      role: 'qa',
      persona: 'You are relentless.',
    );
    final toml = buildRoomFile(room(), [bot]).toml;
    expect(toml, contains('You are relentless.'));
    expect(toml, contains('try to break the work'));
  });

  /// A key in the file would leak the moment it is shared or logged.
  test('an API key goes in the environment, never in the file', () {
    final bot = BotDef(
      name: 'd',
      handle: 'd',
      provider: ProviderKind.deepseek,
      apiKey: 'sk-secret-value',
    );
    final file = buildRoomFile(room(), [bot]);
    expect(file.toml, isNot(contains('sk-secret-value')));
    expect(file.environment['DEEPSEEK_API_KEY'], 'sk-secret-value');
  });

  test('a DeepSeek bot with no key is skipped with a reason', () {
    final file = buildRoomFile(room(), [
      BotDef(name: 'd', handle: 'd', provider: ProviderKind.deepseek),
    ]);
    expect(file.skipped.single, contains('needs an API key'));
    expect(
      file.toml,
      isNot(contains('deepseek')),
      reason: 'an unusable bot must not be half-written into the file',
    );
  });

  test(
    'quotes and newlines in a goal are escaped, not left to break the parse',
    () {
      final r = RoomDef(name: 'r', goal: 'Ship "v2"\nwith notes');
      final toml = buildRoomFile(r, [BotDef(name: 'a', handle: 'a')]).toml;
      expect(toml, contains(r'\"v2\"'));
      expect(toml, isNot(contains('Ship "v2"\nwith')));
    },
  );

  test('a name with spaces becomes a usable handle', () {
    final file = buildRoomFile(room(), [
      BotDef(name: 'Big Bot Name', handle: ''),
    ]);
    expect(file.toml, contains('handle = "big-bot-name"'));
  });

  test('tasks are written as seed board entries', () {
    final file = buildRoomFile(room(tasks: ['Draft the notes']), [
      BotDef(name: 'a', handle: 'a'),
    ]);
    expect(file.toml, contains('[[tasks]]'));
    expect(file.toml, contains('Draft the notes'));
  });

  group('a device with no CLIs', () {
    /// Signed in the way the app really is: through the stored-credential
    /// API, not by reaching past it.
    Future<Credentials> signedInTo(ProviderKind provider) async {
      SharedPreferences.setMockInitialValues({});
      // Secrets live in the keystore now, which needs a stand-in under test.
      FlutterSecureStorage.setMockInitialValues({});
      final credentials = Credentials();
      await credentials.importBundle(
        jsonEncode({
          'version': 1,
          'source': 'test',
          'providers': [
            {
              'provider': provider.key,
              'token': 'tok-' + provider.key,
              'kind': 'bearer',
              'plan': 'pro',
            },
          ],
        }),
      );
      return credentials;
    }

    test('a signed-in Codex bot runs on the API, not a CLI', () async {
      final file = buildRoomFile(
        room(),
        [
          BotDef(
            name: 'a',
            handle: 'a',
            provider: ProviderKind.codex,
            model: 'gpt-5.6-sol',
          ),
        ],
        credentials: await signedInTo(ProviderKind.codex),
        canRunClis: false,
      );
      expect(file.toml, contains('kind = "openai"'));
      expect(file.toml, contains('profile = "codex"'));
      expect(file.toml, isNot(contains('cli_agent')));
      // A ChatGPT session is not an OpenAI API key, so it travels under its
      // own name to the adapter that speaks ChatGPT's backend.
      expect(file.environment['CODEX_AUTH_TOKEN'], 'tok-codex');
      expect(file.environment.containsKey('OPENAI_API_KEY'), isFalse);
      expect(file.skipped, isEmpty);
    });

    test('a signed-in Grok bot runs on the xAI API', () async {
      final file = buildRoomFile(
        room(),
        [
          BotDef(
            name: 'g',
            handle: 'g',
            provider: ProviderKind.grok,
            model: 'grok-4.6',
          ),
        ],
        credentials: await signedInTo(ProviderKind.grok),
        canRunClis: false,
      );
      expect(file.toml, contains('profile = "grok"'));
      expect(file.environment['GROK_AUTH_TOKEN'], 'tok-grok');
      expect(file.environment.containsKey('GROK_API_KEY'), isFalse);
      expect(file.skipped, isEmpty);
    });

    test(
      'blank mobile models are caught before reaching a signed-in provider',
      () async {
        for (final provider in [ProviderKind.codex, ProviderKind.grok]) {
          final file = buildRoomFile(
            room(),
            [BotDef(name: 'a', provider: provider)],
            credentials: await signedInTo(provider),
            canRunClis: false,
          );
          expect(file.skipped.single, contains('no model is selected'));
          expect(file.toml, isNot(contains('[[bots]]')));
        }
        final file = buildRoomFile(
          room(),
          [BotDef(name: 'demo', provider: ProviderKind.scripted)],
          userName: 'Alex',
          userProfile: 'Founder. Ask about geography first.',
        );
        expect(file.toml, contains('Alex'));
        expect(file.toml, contains('Ask about geography first.'));
      },
    );

    test(
      'Anthropic is left out, because it has no sign-in that works here',
      () async {
        final file = buildRoomFile(
          room(),
          [BotDef(name: 'c', handle: 'c', provider: ProviderKind.anthropic)],
          credentials: await signedInTo(ProviderKind.anthropic),
          canRunClis: false,
        );
        expect(file.toml, isNot(contains('[[bots]]')));
        expect(file.skipped.single, contains('only run'));
      },
    );

    test('a provider that is not signed in says to sign in', () async {
      final file = buildRoomFile(room(), [
        BotDef(name: 'a', handle: 'a', provider: ProviderKind.codex),
      ], canRunClis: false);
      expect(file.toml, isNot(contains('[[bots]]')));
      expect(file.skipped.single, contains('Sign in'));
    });

    test('a token is never written into the room file itself', () async {
      final file = buildRoomFile(
        room(),
        [
          BotDef(
            name: 'a',
            handle: 'a',
            provider: ProviderKind.codex,
            model: 'm',
          ),
        ],
        credentials: await signedInTo(ProviderKind.codex),
        canRunClis: false,
      );
      expect(file.toml, isNot(contains('tok-codex')));
    });
  });
}

// ------------------------------------------------------------- the role table
//
// Without one the engine matches roles by name exactly, and work sits stranded
// whenever the single bot holding a role is busy — which is how a sixteen-bot
// room ended up with nine bots free and nothing moving.

void _roleTableTests() {
  group('the role table', () {
    test('roles the room holds are declared with their stand-ins', () {
      final file = buildRoomFile(room(), [
        BotDef(name: 'alice', provider: ProviderKind.scripted, role: 'ops'),
        BotDef(name: 'bob', provider: ProviderKind.scripted, role: 'engineer'),
        BotDef(name: 'carol', provider: ProviderKind.scripted, role: 'backend'),
      ]);

      expect(file.toml, contains('[[roles]]'));
      expect(file.toml, contains('name = "ops"'));
      expect(file.toml, contains('capabilities = ["infra", "deploy"]'));
      expect(
        file.toml,
        contains('fallbacks = ["backend", "engineer"]'),
        reason: 'ops work should be coverable when its one holder is busy',
      );
    });

    /// Pointing at a role nobody holds does nothing but clutter the file.
    test('stand-ins nobody holds are left out', () {
      final file = buildRoomFile(room(), [
        BotDef(name: 'alice', provider: ProviderKind.scripted, role: 'ops'),
      ]);

      expect(file.toml, contains('name = "ops"'));
      expect(
        file.toml,
        isNot(contains('fallbacks')),
        reason: 'neither backend nor engineer is in this room',
      );
    });

    /// The widest substitution in the catalogue, and a deliberate one.
    test('security can be covered by a reviewer or an architect', () {
      final file = buildRoomFile(room(), [
        BotDef(
          name: 'alice',
          provider: ProviderKind.scripted,
          role: 'security',
        ),
        BotDef(name: 'bob', provider: ProviderKind.scripted, role: 'reviewer'),
        BotDef(
          name: 'carol',
          provider: ProviderKind.scripted,
          role: 'architect',
        ),
      ]);

      final section = file.toml
          .split('[[roles]]')
          .firstWhere((part) => part.contains('name = "security"'));
      expect(section, contains('capabilities = ["audit", "review"]'));
      expect(
        section,
        contains('fallbacks = ["reviewer", "architect"]'),
        reason: 'reviewer first — it is the only other role carrying `audit`',
      );
    });

    /// A room with no reviewer or architect still leaves security to its own
    /// holder rather than naming stand-ins that are not there.
    test('security stand-ins are only named when the room has them', () {
      final file = buildRoomFile(room(), [
        BotDef(
          name: 'alice',
          provider: ProviderKind.scripted,
          role: 'security',
        ),
        BotDef(name: 'bob', provider: ProviderKind.scripted, role: 'engineer'),
      ]);

      final section = file.toml
          .split('[[roles]]')
          .firstWhere((part) => part.contains('name = "security"'));
      expect(section, isNot(contains('fallbacks')));
    });
  });
}
