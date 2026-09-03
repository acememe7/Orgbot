// You must be able to talk to a room and get an answer. These cover the three
// bugs that made the app a read-only log viewer: a human message that no bot
// was ever woken to read, a resume that never restarted the loop, and an idle
// room that could not be revived.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/engine/engine_core.dart';
import 'package:orgbot_app/engine/runner.dart';
import 'package:orgbot_app/state/local_room_controller.dart';

/// A bot that answers whenever it is given a turn, and records what it saw.
class EchoBot implements BotProvider {
  final String handle;
  final List<String> seen = [];
  EchoBot(this.handle);

  @override
  String get name => handle;

  @override
  Future<BotTurn> act({required String situation}) async {
    seen.add(situation);
    if (situation.contains('ping')) {
      return const BotTurn(text: 'pong');
    }
    // Nothing to say: an empty turn, so the room settles instead of chattering.
    return const BotTurn();
  }
}

LocalRoomController controllerWith(EchoBot bot) {
  final engine = Engine(name: 'chat', goal: 'answer the person');
  final runner = Runner(engine, manual: true);
  final c = LocalRoomController(engine: engine, runner: runner);
  c.seat(bot.handle, const ['writer'], bot);
  return c;
}

void main() {
  test('a human message wakes a bot that had nothing to do', () async {
    final bot = EchoBot('helper');
    final c = controllerWith(bot);

    // With an empty board and no messages, nobody is actionable.
    await c.start();
    expect(c.runner.nextActor(), isNull,
        reason: 'a quiet room should settle rather than spin');
    final turnsBefore = bot.seen.length;

    // The person speaks; a bot must be given a turn to answer.
    await c.say('ping');

    expect(bot.seen.length, greaterThan(turnsBefore),
        reason: 'the bot must be woken to read what was said');
    expect(bot.seen.last, contains('ping'),
        reason: 'the message must reach the situation digest');
    expect(
      c.engine.messages.map((m) => m.body),
      contains('pong'),
      reason: 'the answer must land in the transcript',
    );
  });

  test('the human message is attributed to the person, not a bot', () async {
    final bot = EchoBot('helper');
    final c = controllerWith(bot);
    c.speakerName = 'Alex';
    await c.start();
    await c.say('ping');

    final human = c.engine.messages.firstWhere((m) => m.body == 'ping');
    expect(human.author, 'Alex');
  });

  test('an empty message is ignored rather than waking the room', () async {
    final bot = EchoBot('helper');
    final c = controllerWith(bot);
    await c.start();
    final before = bot.seen.length;

    await c.say('   ');

    expect(c.engine.messages, isEmpty);
    expect(bot.seen.length, before);
  });

  test('resume restarts the loop after a pause', () async {
    final bot = EchoBot('helper');
    final c = controllerWith(bot);
    await c.start();

    c.pause();
    expect(c.state, LocalRunState.paused);

    c.resume();
    // Resume kicks the loop off again; let it settle.
    await Future<void>.delayed(const Duration(milliseconds: 50));

    expect(c.engine.phase, RoomPhase.running,
        reason: 'a resumed room must actually be running');
    expect(c.state, isNot(LocalRunState.paused),
        reason: 'the loop must have restarted, not just flipped a flag');
  });

  test('a room can still be talked to after it has gone idle', () async {
    final bot = EchoBot('helper');
    final c = controllerWith(bot);

    await c.start();
    expect(c.isAwaitingYou, isTrue,
        reason: 'idle is a resting state you can speak into, not an ending');

    // Two separate conversations, after the loop has already returned twice.
    await c.say('ping');
    final first = c.engine.messages.length;
    await c.say('ping again');

    expect(c.engine.messages.length, greaterThan(first),
        reason: 'the room must stay responsive across repeated messages');
  });

  test('speaking resets the idle counter so the room is not called stalled',
      () async {
    final bot = EchoBot('helper');
    final c = controllerWith(bot);
    await c.start();

    c.runner.idleRounds = Runner.maxIdleRounds;
    await c.say('ping');

    expect(c.runner.idleRounds, lessThan(Runner.maxIdleRounds),
        reason: 'a fresh question deserves a fresh budget');
    expect(c.engine.messages.map((m) => m.body), contains('pong'));
  });
}
