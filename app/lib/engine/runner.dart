// The runner: round-robin turns over the on-device engine, mirroring
// gptbots-runtime's orchestrator. Pure Dart, no I/O.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'engine_core.dart';

/// One tool call a bot wants to make this turn.
class ToolCall {
  final String name;
  final Map<String, Object?> args;
  const ToolCall(this.name, [this.args = const {}]);
}

/// What a provider returns for one model step: tools to run (and results fed
/// back) and/or the prose to post.
class BotTurn {
  final List<ToolCall> tools;
  final String? text;
  const BotTurn({this.tools = const [], this.text});
}

/// A bot's backend: the thing that reads the room and decides. On Android this
/// is a model call; in tests it is scripted/reactive.
abstract class BotProvider {
  /// Given what changed for this bot and the room digest, decide.
  Future<BotTurn> act({required String situation});
  String get name;
}

/// Why a run ended.
enum RunOutcome { completed, idle, stalled, outOfTurns, paused }

extension RunOutcomeX on RunOutcome {
  bool get isSuccess => this == RunOutcome.completed;
}

/// A seated bot: its identity plus its backend and how much it has seen.
class RoomBot {
  final Agent agent;
  final BotProvider provider;
  int lastSeen = 0;
  int stepsUsed = 0;
  String? lastSaid;

  RoomBot(this.agent, this.provider);

  bool get isCoordinator => agent.roles.contains('coordinator');
}

/// Drives a room to completion: who acts next, when, and why it stopped.
class Runner {
  final Engine engine;
  final List<RoomBot> bots = [];
  final bool manual; // manual completion: idle is a resting state, not failure
  int cursor = 0;
  int idleRounds = 0;
  static const int maxIdleRounds = 6;
  static const int maxTotalTurns = 200;
  int turnsUsed = 0;

  Runner(this.engine, {this.manual = false});

  RoomBot seat(String handle, List<String> roles, BotProvider provider,
      {bool mayCreate = false, bool mayInspect = false, bool maySubmit = false}) {
    final agent = Agent(
        handle: handle,
        roles: roles,
        mayCreateTasks: mayCreate,
        mayInspect: mayInspect,
        maySubmit: maySubmit);
    engine.seat(agent);
    final bot = RoomBot(agent, provider);
    bots.add(bot);
    return bot;
  }

  // ---- scheduling ----

  /// Everything the bot has not seen yet, plus the room digest.
  String buildSituation(RoomBot bot) {
    final fresh = engine.log.skip(bot.lastSeen).join('\n');
    final b = StringBuffer('Room: ' + engine.name + '\n' + fresh);
    b.writeln('\nBoard:');
    for (final t in engine.tasks.values) {
      b.writeln('  ' + t.state.name + ' ' + t.id + ' - ' + t.title +
          (t.assignee != null ? ' (assigned to ' + t.assignee! + ')' : ''));
    }
    if (b.toString().trim().isEmpty) return 'nothing has happened yet';
    return b.toString();
  }

  bool actionable(RoomBot bot) {
    final a = bot.agent;
    // Live work in hand keeps the assignee awake until it leaves their hands.
    final held = engine.tasks.values
        .where((t) => t.assignee == a.id && !t.state.isTerminal)
        .toList();
    if (held.any((t) => t.state != TaskState.inReview && t.state != TaskState.approved)) {
      return true;
    }
    // Work offered to this bot, still offered.
    final me = engine.handoffs.values.where((h) {
      final addressed = h.toAgent == a.id ||
          (h.toRole != null && a.roles.contains(h.toRole));
      return addressed && h.state == HandoffState.offered;
    });
    if (me.isNotEmpty) return true;
    // Something to inspect.
    if (a.mayInspect &&
        engine.tasks.values.any((t) => t.state == TaskState.inReview)) {
      return true;
    }
    // Approved work waiting to be delivered.
    if (a.maySubmit &&
        engine.tasks.values.any((t) => t.state == TaskState.approved)) {
      return true;
    }
    // An empty board is a coordinator's cue to plan.
    if (a.mayCreateTasks && engine.tasks.isEmpty) return true;
    // Free, with work available.
    final free = held.isEmpty;
    if (free &&
        engine.tasks.values.any((t) =>
            t.state == TaskState.ready && a.canTake(t.requiredRole))) {
      return true;
    }
    return false;
  }

  RoomBot? nextActor() {
    if (bots.isEmpty) return null;
    for (var i = 0; i < bots.length; i++) {
      final bot = bots[(cursor + i) % bots.length];
      if (actionable(bot)) {
        cursor = (cursor + i + 1) % bots.length;
        return bot;
      }
    }
    return null;
  }

  // ---- one turn ----

  /// Give one bot one turn. Returns false when nobody is actionable.
  Future<bool> step() async {
    final bot = nextActor();
    if (bot == null) return false;
    if (engine.phase != RoomPhase.running) return false;

    var progress = false;
    var steps = 0;
    final results = <String>[];
    while (steps < (bot.agent.maySubmit ? 24 : 12)) {
      steps++;
      bot.stepsUsed++;
      turnsUsed++;
      if (turnsUsed > maxTotalTurns) return false;
      var situation = buildSituation(bot);
      if (results.isNotEmpty) {
        situation = situation + '\n\nYour last actions:\n' + results.join('\n');
      }
      final turn = await bot.provider.act(situation: situation);
      for (final call in turn.tools) {
        final outcome = engine.call(call.name, call.args, bot.agent.id);
        results.add(call.name + ' -> ' + (outcome.isError ? 'error: ' : '') + outcome.content);
        if (!outcome.isError) progress = true;
      }
      final said = turn.text;
      if (said != null && said.trim().isNotEmpty) {
        if (said.trim() != (bot.lastSaid ?? '')) {
          progress = true;
        }
        bot.lastSaid = said.trim();
        engine.call('post_message', {'body': said.trim()}, bot.agent.id);
        bot.lastSeen = engine.log.length;
      }
      // The bot keeps acting while it asks for more tools; once it answers
      // with prose (or nothing) the turn is done.
      if (turn.tools.isEmpty) break;
    }
    bot.lastSeen = engine.log.length;
    engine.turnsUsed = turnsUsed;
    if (progress) {
      idleRounds = 0;
    } else {
      idleRounds++;
    }
    return true;
  }

  /// Run until the room completes, idles, stalls, or runs out of turns.
  Future<RunOutcome> run() async {
    if (engine.phase == RoomPhase.idle) {
      engine.start();
    }
    while (true) {
      if (engine.phase == RoomPhase.paused) return RunOutcome.paused;
      if (engine.phase != RoomPhase.running) {
        return RunOutcome.completed;
      }
      if (turnsUsed > maxTotalTurns) return RunOutcome.outOfTurns;
      if (idleRounds >= maxIdleRounds) return RunOutcome.stalled;
      if (allTasksTerminal(engine)) {
        engine.phase = RoomPhase.completed;
        return RunOutcome.completed;
      }
      final acted = await step();
      if (!acted) {
        if (manual) return RunOutcome.idle;
        if (allTasksTerminal(engine)) {
          engine.phase = RoomPhase.completed;
          return RunOutcome.completed;
        }
        return RunOutcome.idle;
      }
    }
  }
}
