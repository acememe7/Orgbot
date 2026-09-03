// Tests for the on-device coordination engine: the same invariants the Rust
// engine asserts — separation of duties, one claim per task, review before
// submit, handoffs addressed to the right bot — enforced in Dart.
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/engine/engine_core.dart';

Engine room({bool workerSubmits = true}) {
  final e = Engine(name: 'r', goal: 'ship it');
  e.seat(Agent(
    handle: 'lead',
    roles: const ['coordinator'],
    mayCreateTasks: true,
    maySubmit: true,
  ));
  e.seat(Agent(
    handle: 'worker',
    roles: const ['writer'],
    maySubmit: workerSubmits,
  ));
  e.seat(Agent(
    handle: 'worker-b',
    roles: const ['writer'],
  ));
  e.seat(Agent(
    handle: 'qa',
    roles: const ['reviewer'],
    mayInspect: true,
  ));
  e.start();
  return e;
}

String taskId(Engine e) => e.tasks.values.first.id;

ToolOutcome seedTask(Engine e, {String role = 'writer'}) {
  final lead = e.agents.values.firstWhere((a) => a.handle == 'lead').id;
  return e.call(
    'create_task',
    {'title': 'write it', 'brief': 'do it well', 'required_role': role},
    lead,
  );
}

void main() {
  test('a full cycle runs: claim, work, review, submit', () {
    final e = room();
    expect(seedTask(e).isError, isFalse);
    final id = taskId(e);
    expect(e.tasks[id]!.state, TaskState.ready);

    final worker = e.agents.values.firstWhere((a) => a.handle == 'worker').id;
    expect(e.call('claim_task', {'task': id}, worker).content, contains('claimed'));
    expect(e.tasks[id]!.state, TaskState.claimed);
    expect(e.tasks[id]!.assignee, worker);

    expect(e.call('start_task', {'task': id}, worker).isError, isFalse);
    expect(e.tasks[id]!.state, TaskState.inProgress);

    expect(e.call('add_artifact', {'task': id, 'body': 'the work'}, worker).isError, isFalse);
    expect(e.call('submit_for_review', {'task': id}, worker).isError, isFalse);
    expect(e.tasks[id]!.state, TaskState.inReview);

    final qa = e.agents.values.firstWhere((a) => a.handle == 'qa').id;
    final review = e.call('record_review', {'task': id, 'summary': 'good'}, qa);
    expect(review.isError, isFalse, reason: review.content);
    expect(e.tasks[id]!.state, TaskState.approved);

    expect(e.call('submit_task', {'task': id}, worker).isError, isFalse);
    expect(e.tasks[id]!.state, TaskState.submitted);
    expect(allTasksTerminal(e), isTrue);
  });

  test('a task cannot be claimed twice', () {
    final e = room();
    seedTask(e);
    final id = taskId(e);
    final worker = e.agents.values.firstWhere((a) => a.handle == 'worker').id;
    final second = e.agents.values.firstWhere((a) => a.handle == 'worker-b').id;

    expect(e.call('claim_task', {'task': id}, worker).isError, isFalse);
    final again = e.call('claim_task', {'task': id}, second);
    expect(again.isError, isTrue);
    expect(e.tasks[id]!.assignee, worker);
  });

  test('role mismatch blocks a claim', () {
    final e = room();
    seedTask(e, role: 'reviewer'); // writers cannot take it
    final id = taskId(e);
    final worker = e.agents.values.firstWhere((a) => a.handle == 'worker').id;
    expect(e.call('claim_task', {'task': id}, worker).isError, isTrue);
  });

  test('separation of duties: workers cannot create, review, or submit', () {
    final e = room(workerSubmits: false);
    final worker = e.agents.values.firstWhere((a) => a.handle == 'worker').id;
    expect(
      e.call('create_task', {'title': 'x', 'brief': 'y'}, worker).isError,
      isTrue,
    );

    // Drive a task into review, then let the non-inspector try to review it.
    final lead = e.agents.values.firstWhere((a) => a.handle == 'lead').id;
    e.call('create_task', {'title': 'x', 'brief': 'y', 'required_role': 'writer'}, lead);
    final id = taskId(e);
    e.call('claim_task', {'task': id}, worker);
    e.call('start_task', {'task': id}, worker);
    e.call('add_artifact', {'task': id, 'body': 'done'}, worker);
    e.call('submit_for_review', {'task': id}, worker);
    final asWorker = e.call('record_review', {'task': id, 'summary': 'fine'}, worker);
    expect(asWorker.isError, isTrue, reason: 'a writer cannot inspect');
  });

  test('no one inspects their own work', () {
    final e = Engine(name: 'solo', goal: 'self check');
    final solo = Agent(
      handle: 'solo',
      roles: const ['writer', 'reviewer'],
      mayCreateTasks: true,
      mayInspect: true,
      maySubmit: true,
    );
    e.seat(solo);
    e.start();
    e.call('create_task', {'title': 'x', 'brief': 'y'}, solo.id);
    final id = taskId(e);
    e.call('claim_task', {'task': id}, solo.id);
    e.call('start_task', {'task': id}, solo.id);
    e.call('add_artifact', {'task': id, 'body': 'mine'}, solo.id);
    e.call('submit_for_review', {'task': id}, solo.id);
    final review = e.call('record_review', {'task': id, 'summary': 'great'}, solo.id);
    expect(review.isError, isTrue);
    expect(review.content, contains('own work'));
  });

  test('major findings send work back and bump the revision', () {
    final e = room();
    seedTask(e);
    final id = taskId(e);
    final worker = e.agents.values.firstWhere((a) => a.handle == 'worker').id;
    final qa = e.agents.values.firstWhere((a) => a.handle == 'qa').id;
    e.call('claim_task', {'task': id}, worker);
    e.call('start_task', {'task': id}, worker);
    e.call('add_artifact', {'task': id, 'body': 'v1'}, worker);
    e.call('submit_for_review', {'task': id}, worker);

    final verdict = e.call('record_review', {
      'task': id,
      'summary': 'fix the naming',
      'findings': [
        {'severity': 'major', 'summary': 'naming', 'locus': 'artifact'},
      ],
    }, qa);
    expect(verdict.isError, isFalse);
    expect(e.tasks[id]!.state, TaskState.inProgress);
    expect(e.tasks[id]!.revision, 1);
  });

  test('handoffs are addressed, and only the recipient may accept', () {
    final e = room();
    seedTask(e);
    final id = taskId(e);
    final worker = e.agents.values.firstWhere((a) => a.handle == 'worker').id;
    final qa = e.agents.values.firstWhere((a) => a.handle == 'qa').id;
    final other = e.agents.values.firstWhere((a) => a.handle == 'worker-b').id;

    e.call('claim_task', {'task': id}, worker);
    final offered = e.call('offer_handoff', {
      'task': id,
      'to_role': 'reviewer',
      'reason': 'your eyes on this',
    }, worker);
    expect(offered.isError, isFalse);
    final hid = offered.content.split(' ').first;

    // A bot the offer was not addressed to cannot accept.
    final wrong = e.call('accept_handoff', {'handoff': hid}, other);
    expect(wrong.isError, isTrue);

    // The recipient can; ownership moves.
    expect(e.call('accept_handoff', {'handoff': hid}, qa).isError, isFalse);
    expect(e.tasks[id]!.assignee, qa);
    expect(e.handoffs[hid]!.state, HandoffState.accepted);

    // Only the sender completes; completing too early is rejected, so move on.
    final premature = e.call('complete_handoff', {'handoff': hid}, qa);
    expect(premature.isError, isTrue);
    expect(e.call('complete_handoff', {'handoff': hid}, worker).isError, isFalse);
    expect(e.handoffs[hid]!.state, HandoffState.completed);
  });

  test('a room that has not started rejects every move', () {
    final e = Engine(name: 'cold', goal: 'not started');
    e.seat(Agent(handle: 'lead', roles: const ['coordinator'], mayCreateTasks: true));
    final result = e.call('create_task', {'title': 'x', 'brief': 'y'}, e.agents.keys.first);
    expect(result.isError, isTrue);
    expect(result.content, contains('running'));
  });

  test('unknown tools are reported, not swallowed', () {
    final e = room();
    final worker = e.agents.values.firstWhere((a) => a.handle == 'worker').id;
    expect(e.call('teleport', {}, worker).content, contains('no tool'));
  });
}
