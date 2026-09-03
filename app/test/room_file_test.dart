// The room file the app generates has to be one the engine will actually run.
// A file that only looks right fails at `serve` time with a parse error, so
// these assert the shape the Rust config expects.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/room_file.dart';
import 'package:orgbot_app/orgbot/roles.dart';

RoomDef room({List<String> tasks = const []}) => RoomDef(
      name: 'Release notes',
      goal: 'Write the 1.3 notes',
      path: '/tmp/project',
      tasks: tasks,
    );

void main() {
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

  test('each CLI maps to its own binary', () {
    for (final pair in [
      (ProviderKind.anthropic, 'claude'),
      (ProviderKind.codex, 'codex'),
      (ProviderKind.grok, 'grok'),
    ]) {
      final file = buildRoomFile(
          room(), [BotDef(name: 'b', handle: 'b', provider: pair.$1)]);
      expect(file.toml, contains('cli = "${pair.$2}"'));
    }
  });

  test('the role decides the permissions written into the file', () {
    final reviewer = buildRoomFile(room(),
        [BotDef(name: 'q', handle: 'q', role: 'reviewer')]).toml;
    expect(reviewer, contains('may_inspect = true'));
    expect(reviewer, contains('may_submit = false'),
        reason: 'an inspector must not also deliver');

    final lead = buildRoomFile(room(),
        [BotDef(name: 'l', handle: 'l', role: 'coordinator')]).toml;
    expect(lead, contains('may_create_tasks = true'));
    expect(lead, contains('may_submit = true'));
    expect(lead, contains('may_inspect = false'),
        reason: 'a coordinator must not sign off on its own plan');
  });

  test('an explicit power override beats the role default', () {
    final bot = BotDef(name: 'x', handle: 'x', role: 'engineer')
      ..powers = const RolePowers(mayInspect: true);
    expect(buildRoomFile(room(), [bot]).toml, contains('may_inspect = true'));
  });

  test('the role brief is folded into the persona', () {
    final bot = BotDef(
        name: 'x', handle: 'x', role: 'qa', persona: 'You are relentless.');
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
    final file = buildRoomFile(
        room(), [BotDef(name: 'd', handle: 'd', provider: ProviderKind.deepseek)]);
    expect(file.skipped.single, contains('needs an API key'));
    expect(file.toml, isNot(contains('deepseek')),
        reason: 'an unusable bot must not be half-written into the file');
  });

  test('quotes and newlines in a goal are escaped, not left to break the parse', () {
    final r = RoomDef(name: 'r', goal: 'Ship "v2"\nwith notes');
    final toml = buildRoomFile(r, [BotDef(name: 'a', handle: 'a')]).toml;
    expect(toml, contains(r'\"v2\"'));
    expect(toml, isNot(contains('Ship "v2"\nwith')));
  });

  test('a name with spaces becomes a usable handle', () {
    final file =
        buildRoomFile(room(), [BotDef(name: 'Big Bot Name', handle: '')]);
    expect(file.toml, contains('handle = "big-bot-name"'));
  });

  test('tasks are written as seed board entries', () {
    final file = buildRoomFile(room(tasks: ['Draft the notes']),
        [BotDef(name: 'a', handle: 'a')]);
    expect(file.toml, contains('[[tasks]]'));
    expect(file.toml, contains('Draft the notes'));
  });
}
