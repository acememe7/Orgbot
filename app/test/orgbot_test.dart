// Model + persistence + seed tests for the OrgBot phone app.
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/engine/engine_core.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';

void main() {
  test('bots and rooms round-trip through json', () {
    final bot = BotDef(
      name: 'Writer',
      handle: 'writer',
      persona: 'concise prose',
      provider: ProviderKind.anthropic,
      model: 'claude-sonnet-5',
    );
    final fromJson = BotDef.fromJson(bot.toJson());
    expect(fromJson.name, 'Writer');
    expect(fromJson.provider, ProviderKind.anthropic);
    expect(fromJson.model, 'claude-sonnet-5');
    expect(fromJson.id, bot.id);

    final room = RoomDef(
      name: 'notes',
      goal: 'write notes',
      tasks: ['cover every change'],
      bots: [RoomSlot(bot.id)],
    );
    final room2 = RoomDef.fromJson(room.toJson());
    expect(room2.name, 'notes');
    expect(
      room2.bots.single.botId,
      bot.id,
      reason: 'a slot names the bot; the role travels with the bot itself',
    );
  });

  test('the library persists bots and rooms through a memory saver', () async {
    final saver = MemorySaver();
    final lib = Library(saver: saver);
    await lib.load();
    lib.userName = 'Alex';
    lib.userProfile = 'I own the product. Ask before choosing a target market.';
    lib.addBot(
      BotDef(
        name: 'QA',
        handle: 'qa',
        persona: 'checks',
        provider: ProviderKind.scripted,
      ),
    );
    lib.addRoom(
      RoomDef(name: 'r', goal: 'g', bots: [RoomSlot(lib.bots.first.id)]),
    );
    await Future<void>.delayed(Duration.zero); // let persist() settle

    final reloaded = Library(saver: saver);
    await reloaded.load();
    expect(reloaded.bots.single.name, 'QA');
    expect(reloaded.userName, 'Alex');
    expect(reloaded.userProfile, contains('target market'));
    expect(reloaded.rooms.single.bots.single.botId, reloaded.bots.single.id);
  });

  test(
    'removing a bot preserves its rooms and removes only the seat',
    () async {
      final lib = Library(saver: MemorySaver());
      await lib.load();
      final bot = lib.addBot(BotDef(name: 'W'));
      lib.addRoom(RoomDef(name: 'r', bots: [RoomSlot(bot.id)]));
      lib.removeBot(bot.id);
      expect(lib.bots, isEmpty);
      expect(lib.rooms.single.name, 'r');
      expect(lib.rooms.single.bots, isEmpty);
    },
  );

  test('seeding a task puts it on the board in ready state', () {
    final engine = Engine(name: 'r', goal: 'g');
    final id = engine.seedTask('t', 'do it', requiredRole: 'writer');
    expect(engine.tasks[id]!.state, TaskState.ready);
    expect(engine.tasks[id]!.requiredRole, 'writer');
  });
}
