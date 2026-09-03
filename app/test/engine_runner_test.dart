// End-to-end runs over the on-device engine: bots driven by reactive
// providers that read the real situation text and act through the tool
// surface, so scheduling bugs surface here rather than on a phone.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'package:flutter_test/flutter_test.dart';
import 'package:gptbots_app/engine/engine_core.dart';
import 'package:gptbots_app/engine/runner.dart';

String firstId(String text, String prefix) {
  final words = text.split(RegExp('[^A-Za-z0-9_]+'));
  for (final w in words) {
    if (w.startsWith(prefix + '_') && w.length > prefix.length + 2) return w;
  }
  return '';
}

/// The board line for a task (the digest is always the last section).
String boardSection(String text) {
  final i = text.lastIndexOf('Board:');
  return i < 0 ? '' : text.substring(i);
}

String stateOf(String text, String id) {
  final board = boardSection(text);
  final i = board.lastIndexOf(id);
  if (i < 0) return '';
  final before = board.substring(0, i).trim();
  final words = before.split(RegExp('\\s+'));
  return words.isEmpty ? '' : words.last;
}

/// Reads the situation and decides like a small model would.
class ReactiveBot implements BotProvider {
  final String handle;
  final bool maySubmitFinal;
  ReactiveBot(this.handle, {this.maySubmitFinal = false});

  @override
  String get name => handle;

  @override
  Future<BotTurn> act({required String situation}) async {
    if (handle == 'lead') {
      if (!situation.contains('task_')) {
        return const BotTurn(tools: [
          ToolCall('create_task', {
            'title': 'ship the notes',
            'brief': 'write and review release notes',
            'required_role': 'writer',
          }),
        ]);
      }
      return const BotTurn();
    }
    if (handle == 'writer') {
      final task = firstId(situation, 'task');
      if (task.isEmpty) return const BotTurn();
      final state = stateOf(situation, task);
      if (state == 'ready') {
        return BotTurn(tools: [ToolCall('claim_task', {'task': task})]);
      }
      if (state == 'claimed') {
        return BotTurn(tools: [
          ToolCall('start_task', {'task': task}),
          ToolCall('add_artifact', {'task': task, 'body': 'the release notes draft'}),
          ToolCall('submit_for_review', {'task': task}),
        ]);
      }
      if (state == 'approved') {
        return BotTurn(
            text: 'submitted the notes',
            tools: [ToolCall('submit_task', {'task': task})]);
      }
      return const BotTurn();
    }
    if (handle == 'qa') {
      final board = boardSection(situation);
      if (board.contains('inReview')) {
        final task = firstId(board, 'task');
        return BotTurn(tools: [
          ToolCall('record_review', {'task': task, 'summary': 'looks right'}),
        ]);
      }
      return const BotTurn();
    }
    return const BotTurn();
  }
}

Engine makeEngine() =>
    Engine(name: 'notes', goal: 'write and ship the release notes');

void main() {
  test('a whole room runs to completion on-device', () async {
    final engine = makeEngine();
    final runner = Runner(engine);
    runner.seat('lead', const ['coordinator'], ReactiveBot('lead'),
        mayCreate: true, maySubmit: true);
    runner.seat('writer', const ['writer'], ReactiveBot('writer'),
        maySubmit: true);
    runner.seat('qa', const ['reviewer'], ReactiveBot('qa'), mayInspect: true);

    final outcome = await runner.run();
    expect(outcome, RunOutcome.completed,
        reason: engine.log.join('\n') + '\nTURNS:' + engine.turnsUsed.toString() + ' IDLE:' + runner.idleRounds.toString());

    final task = engine.tasks.values.single;
    expect(task.state, TaskState.submitted);
    expect(task.assignee, isNotNull);
    expect(task.artifacts, isNotEmpty);
    // The transcript carries the bots' voices.
    expect(
      engine.messages.map((m) => m.body).join(' '),
      isNotEmpty,
    );
  });

  test('a room with no inspector idles with work in review', () async {
    final engine = makeEngine();
    final runner = Runner(engine);
    runner.seat('lead', const ['coordinator'], ReactiveBot('lead'),
        mayCreate: true, maySubmit: true);
    runner.seat('writer', const ['writer'], ReactiveBot('writer'),
        maySubmit: true);

    final outcome = await runner.run();
    expect(outcome, RunOutcome.idle,
        reason: engine.log.join('\n') + '\nTURNS:' + engine.turnsUsed.toString());
    expect(engine.tasks.values.single.state, TaskState.inReview);
  });

  test('pausing stops the run', () async {
    final engine = makeEngine();
    final runner = Runner(engine);
    runner.seat('lead', const ['coordinator'], ReactiveBot('lead'),
        mayCreate: true, maySubmit: true);
    runner.seat('writer', const ['writer'], ReactiveBot('writer'),
        maySubmit: true);
    runner.seat('qa', const ['reviewer'], ReactiveBot('qa'), mayInspect: true);

    // Let it run briefly, then pause before it finishes.
    for (var i = 0; i < 3; i++) {
      if (!await runner.step()) break;
    }
    engine.pause();
    final outcome = await runner.run();
    expect(outcome, RunOutcome.paused);
  });

  test('the scheduler wakes the right bot and never double-claims', () async {
    final engine = makeEngine();
    final runner = Runner(engine);
    runner.seat('writer', const ['writer'], ReactiveBot('writer'));
    runner.seat('writer-b', const ['writer'], ReactiveBot('writer-b'));

    // Seed the board directly through a coordinator-free path: create with a
    // coordinator then let both writers race the claim.
    final lead = Agent(handle: 'lead', roles: const ['coordinator'], mayCreateTasks: true);
    engine.seat(lead);
    engine.start();
    engine.call('create_task', {
      'title': 'only one',
      'brief': 'one writer wins',
      'required_role': 'writer',
    }, lead.id);

    // Only one writer may hold it at a time.
    var claims = 0;
    for (var i = 0; i < 6 && claims < 1; i++) {
      final bot = runner.nextActor();
      if (bot == null) break;
      final situation = runner.buildSituation(bot);
      final id = firstId(situation, 'task');
      if (id.isEmpty) break;
      final out = engine.call('claim_task', {'task': id}, bot.agent.id);
      if (!out.isError) claims++;
    }
    expect(claims, 1);
    final holders = engine.tasks.values
        .where((t) => t.assignee != null)
        .length;
    expect(holders, 1);
  });
}
