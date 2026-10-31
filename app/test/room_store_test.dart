/// The store must fold the same event JSON the Rust server emits into a
/// coherent view. These feed it real wire-shaped maps and assert the result,
/// so a drift between the two protocols shows up here rather than at runtime.
library;

import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/net/room_client.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/state/room_store.dart';

/// A fake client that lets a test push server messages by hand.
class FakeClient implements RoomClient {
  final _messages = StreamController<Map<String, Object?>>.broadcast();
  final _state = StreamController<LinkState>.broadcast();
  final sent = <Map<String, Object?>>[];

  void emit(Map<String, Object?> message) => _messages.add(message);

  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;
  @override
  Stream<LinkState> get connectionState => _state.stream;
  @override
  LinkState get current => LinkState.connected;

  @override
  void send(Map<String, Object?> message) => sent.add(message);
  @override
  void sendMessage(
    String author,
    String body, {
    String? taskId,
    String? replyTo,
  }) => sent.add({'type': 'send_message', 'author': author, 'body': body});
  @override
  void answerBot(String taskId, String answer) {}
  @override
  void answerDecision(String decision, String chosen, {String note = ''}) =>
      answered.add((decision: decision, chosen: chosen));
  final answered = <({String decision, String chosen})>[];
  @override
  void pause({String reason = ''}) => sent.add({'type': 'pause'});
  @override
  void stopNow({String reason = ''}) => sent.add({'type': 'stop_now'});
  @override
  void resume() => sent.add({'type': 'resume'});
  @override
  void resync() => sent.add({'type': 'resync'});
  @override
  Future<void> connect(String url, {String? token}) async {}
  @override
  Future<void> close() async {}
}

Map<String, Object?> event(Map<String, Object?> inner, {int seq = 1}) => {
  'type': 'event',
  'envelope': {
    'id': 'ev_x',
    'seq': seq,
    'room': 'room_1',
    'at': 0,
    'event': inner,
  },
};

void main() {
  test('chat messages retain thread and audience metadata', () {
    final message = ChatMessage.fromJson({
      'id': 'msg_reply',
      'author': {'kind': 'human', 'name': 'Alex'},
      'body': 'Following up',
      'intent': 'ask',
      'sent_at': 1,
      'reply_to': 'msg_parent',
      'audience': {
        'kind': 'agents',
        'agents': ['bot_iris'],
      },
    });
    expect(message.replyTo, 'msg_parent');
    expect(message.audienceKind, 'agents');
    expect(message.audienceAgents, ['bot_iris']);
    expect(message.audienceRoles, isEmpty);
  });

  test('local room client serializes an optional reply target', () {
    final client = LocalRoomClient();
    client.sendMessage('Alex', 'Following up', replyTo: 'msg_parent');
    expect(client.sent.single, {
      'type': 'send_message',
      'author': 'Alex',
      'body': 'Following up',
      'reply_to': 'msg_parent',
    });
  });

  test(
    'old time-limit checkpoints show the real stop reason and stay resumable',
    () {
      final client = FakeClient();
      final store = RoomStore(client);
      addTearDown(store.dispose);
      store.restore([
        {
          'type': 'snapshot',
          'room': {
            'id': 'r',
            'spec': {'name': 'Leo'},
            'phase': 'completed',
          },
          'roster': [],
          'tasks': [],
        },
        {
          'type': 'event',
          'envelope': {
            'seq': 1,
            'event': {
              'event': 'room_closed',
              'reason': 'the room ran out of time',
            },
          },
        },
      ]);
      expect(store.runStateLabel, 'Time limit reached');
      expect(store.canResume, isTrue);
      store.restore([
        {'type': 'phase', 'phase': 'running'},
      ]);
      expect(store.runStateLabel, 'Active');
      store.restore([
        {
          'type': 'event',
          'envelope': {
            'seq': 2,
            'event': {
              'event': 'room_closed',
              'reason': "the room's goal was met",
            },
          },
        },
      ]);
      expect(store.runStateLabel, 'Completed');
    },
  );
  test(
    'completed rooms offer Continue and send Resume instead of Pause',
    () async {
      final client = FakeClient();
      final store = RoomStore(client);
      addTearDown(store.dispose);
      store.restore([
        {
          'type': 'snapshot',
          'room': {
            'id': 'room_1',
            'spec': {'name': 'Finished', 'goal': 'old research'},
            'phase': 'completed',
          },
          'roster': [],
          'tasks': [],
        },
      ]);
      expect(store.isCompleted, isTrue);
      expect(store.canResume, isTrue);
      store.togglePause();
      expect(client.sent.last['type'], 'resume');
      store.restore([
        {'type': 'phase', 'phase': 'running'},
      ]);
      expect(store.isCompleted, isFalse);
      store.togglePause();
      expect(client.sent.last['type'], 'pause');
    },
  );
  test(
    'a pause shows as pausing until the turns under way finish; stop now is separate',
    () async {
      final client = FakeClient();
      final store = RoomStore(client);
      addTearDown(store.dispose);
      store.restore([
        {
          'type': 'snapshot',
          'room': {
            'id': 'room_1',
            'spec': {'name': 'Busy', 'goal': 'ship'},
            'phase': 'running',
          },
          'roster': [],
          'tasks': [],
        },
      ]);
      store.togglePause();
      expect(client.sent.last['type'], 'pause');
      expect(store.isPausing, isTrue);
      expect(store.runStateLabel, 'Pausing after current turns');
      store.restore([
        {'type': 'phase', 'phase': 'paused'},
      ]);
      expect(store.isPausing, isFalse);
      expect(store.runStateLabel, 'Paused');

      store.togglePause();
      expect(client.sent.last['type'], 'resume');
      store.restore([
        {'type': 'phase', 'phase': 'running'},
      ]);
      expect(store.isPausing, isFalse);
      store.stopNow();
      expect(client.sent.last['type'], 'stop_now');
    },
  );
  test(
    'old journal failures cannot overwrite repaired snapshot health',
    () async {
      final client = FakeClient();
      final store = RoomStore(client);
      store.restore([
        {
          'type': 'snapshot',
          'head': 2,
          'roster': [
            {
              'id': 'bot_a',
              'handle': 'teo',
              'status': 'idle',
              'error_streak': 0,
            },
          ],
          'tasks': [],
        },
        event({
          'event': 'agent_errored',
          'agent': 'bot_a',
          'error': 'old empty model',
          'attempt': 3,
        }),
        event({
          'event': 'agent_status_changed',
          'agent': 'bot_a',
          'to': 'faulted',
        }, seq: 2),
      ]);
      expect(store.failing, isEmpty);
      expect(store.agents.single.status, AgentStatus.idle);
      store.restore([
        event({
          'event': 'agent_errored',
          'agent': 'bot_a',
          'error': 'new failure',
          'attempt': 1,
        }, seq: 3),
      ]);
      expect(store.failing.single.lastError, 'new failure');
      store.dispose();
    },
  );
  test('a bot reset from faulted stops showing its old failure', () async {
    final client = FakeClient();
    final store = RoomStore(client);
    store.restore([
      {
        'type': 'snapshot',
        'head': 0,
        'roster': [
          {'id': 'bot_e', 'handle': 'ellis', 'status': 'idle'},
        ],
        'tasks': [],
      },
      event({
        'event': 'agent_errored',
        'agent': 'bot_e',
        'error': 'OAuth session expired and could not be refreshed',
        'attempt': 1,
      }, seq: 1),
      event({
        'event': 'agent_status_changed',
        'agent': 'bot_e',
        'from': 'idle',
        'to': 'faulted',
      }, seq: 2),
    ]);
    expect(store.failing.single.lastError, contains('OAuth session expired'));

    // The owner resumes the room, or the room retries the bot by itself.
    store.restore([
      event({
        'event': 'agent_status_changed',
        'agent': 'bot_e',
        'from': 'faulted',
        'to': 'idle',
      }, seq: 3),
    ]);
    expect(store.failing, isEmpty);
    expect(store.agents.single.status, AgentStatus.idle);
    store.dispose();
  });

  test('a delayed stream completion clears the thinking indicator', () async {
    final client = FakeClient();
    final store = RoomStore(client);
    client.emit({
      'type': 'snapshot',
      'roster': [
        {
          'id': 'bot_a',
          'handle': 'iris',
          'display_name': 'Iris',
          'roles': [],
          'status': 'idle',
        },
      ],
      'tasks': [],
    });
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'text_delta', 'text': 'Answer'},
    });
    await Future<void>.delayed(Duration.zero);
    expect(store.activity.single.streaming, isTrue);
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'retry', 'attempt': 1, 'max_retries': 2},
    });
    await Future<void>.delayed(Duration.zero);
    expect(store.activity.single.retryStatus, 'Reconnecting (1/2)');
    expect(store.activity.single.errorStreak, 0);
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'text_delta', 'text': 'Recovered'},
    });
    await Future<void>.delayed(Duration.zero);
    expect(store.activity.single.retryStatus, isNull);
    client.emit(
      event({'event': 'agent_status_changed', 'agent': 'bot_a', 'to': 'idle'}),
    );
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'done', 'stop_reason': 'end_turn'},
    });
    await Future<void>.delayed(Duration.zero);
    expect(store.activity.single.streaming, isFalse);
    store.dispose();
  });
  test(
    'unchanged stream frames do not notify but visible transitions do',
    () async {
      final client = FakeClient();
      final store = RoomStore(client);
      addTearDown(store.dispose);
      client.emit({
        'type': 'snapshot',
        'roster': [
          {
            'id': 'bot_a',
            'handle': 'iris',
            'display_name': 'Iris',
            'roles': [],
            'status': 'idle',
          },
        ],
        'tasks': [],
      });
      await Future<void>.delayed(Duration.zero);

      var notifications = 0;
      store.addListener(() => notifications++);
      client.emit({
        'type': 'stream',
        'agent': 'bot_a',
        'event': {'type': 'text_delta', 'text': 'first'},
      });
      await Future<void>.delayed(Duration.zero);
      expect(notifications, 1);
      for (var i = 0; i < 20; i++) {
        client.emit({
          'type': 'stream',
          'agent': 'bot_a',
          'event': {'type': 'text_delta', 'text': 'delta $i'},
        });
      }
      await Future<void>.delayed(Duration.zero);
      expect(
        notifications,
        1,
        reason: 'plain deltas do not change visible state',
      );

      client.emit({
        'type': 'stream',
        'agent': 'bot_a',
        'event': {'type': 'retry', 'attempt': 1, 'max_retries': 2},
      });
      await Future<void>.delayed(Duration.zero);
      expect(notifications, 2);
      client.emit({
        'type': 'stream',
        'agent': 'bot_a',
        'event': {'type': 'retry', 'attempt': 1, 'max_retries': 2},
      });
      await Future<void>.delayed(Duration.zero);
      expect(notifications, 2, reason: 'the same retry status is not a change');

      client.emit({
        'type': 'stream',
        'agent': 'bot_a',
        'event': {'type': 'text_delta', 'text': 'resumed'},
      });
      await Future<void>.delayed(Duration.zero);
      expect(notifications, 3, reason: 'resuming clears the retry status');
      client.emit({
        'type': 'stream',
        'agent': 'bot_a',
        'event': {'type': 'done'},
      });
      await Future<void>.delayed(Duration.zero);
      expect(notifications, 4);

      client.emit(
        event({
          'event': 'message_posted',
          'message': {
            'id': 'msg_1',
            'room': 'room_1',
            'author': {'kind': 'human', 'name': 'You'},
            'audience': {'kind': 'everyone'},
            'intent': 'say',
            'body': 'ordinary message',
            'sent_at': 0,
          },
        }),
      );
      await Future<void>.delayed(Duration.zero);
      expect(notifications, 5, reason: 'ordinary room events still notify');
      expect(store.messages.single.body, 'ordinary message');
    },
  );
  test(
    'replacement snapshot replays history without duplicates or resync loops',
    () async {
      final client = FakeClient();
      final store = RoomStore(client);
      final posted = event({
        'event': 'message_posted',
        'message': {
          'id': 'msg_1',
          'room': 'room_1',
          'author': {'kind': 'human', 'name': 'You'},
          'audience': {'kind': 'everyone'},
          'intent': 'say',
          'body': 'keep this',
          'sent_at': 0,
        },
      });
      for (var i = 0; i < 2; i++) {
        client.emit({'type': 'snapshot', 'roster': [], 'tasks': [], 'head': 1});
        client.emit(posted);
        client.emit(posted);
        await Future<void>.delayed(Duration.zero);
        expect(store.messages.single.body, 'keep this');
      }
      client.emit({'type': 'error', 'message': 'not allowed'});
      await Future<void>.delayed(Duration.zero);
      expect(store.lastError, 'not allowed');
      expect(client.sent, isEmpty);
    },
  );
  test('a snapshot populates the room, roster, and board', () async {
    final client = FakeClient();
    final store = RoomStore(client);

    client.emit({
      'type': 'snapshot',
      'wire_version': 1,
      'room': {
        'id': 'room_1',
        'spec': {
          'name': 'release',
          'goal': 'ship it',
          'limits': {'max_total_turns': 200},
        },
        'phase': 'running',
        'turns_used': 3,
      },
      'roster': [
        {
          'id': 'bot_a',
          'handle': 'lead',
          'display_name': 'Lead',
          'roles': ['coordinator'],
          'status': 'idle',
        },
        {
          'id': 'bot_b',
          'handle': 'w1',
          'display_name': 'Worker',
          'roles': ['writer'],
          'status': 'working',
        },
      ],
      'tasks': [
        {
          'id': 'task_1',
          'title': 'write notes',
          'brief': 'do it',
          'state': 'in_progress',
          'priority': 'high',
          'assignee': 'bot_b',
          'revision': 0,
        },
      ],
      'head': 5,
    });
    await Future<void>.delayed(Duration.zero);

    expect(store.room?.name, 'release');
    expect(store.room?.phase, RoomPhase.running);
    expect(store.agents.length, 2);
    // The coordinator sorts first.
    expect(store.agents.first.handle, 'lead');
    expect(store.tasks.length, 1);
    expect(store.tasks.first.state, TaskState.inProgress);
    expect(store.handleFor('bot_b'), 'w1');
  });

  test('a posted message lands in the transcript', () async {
    final client = FakeClient();
    final store = RoomStore(client);

    client.emit(
      event({
        'event': 'message_posted',
        'message': {
          'id': 'msg_1',
          'room': 'room_1',
          'author': {'kind': 'agent', 'agent': 'bot_b'},
          'audience': {'kind': 'everyone'},
          'intent': 'say',
          'body': 'on it',
          'sent_at': 0,
        },
      }),
    );
    await Future<void>.delayed(Duration.zero);

    expect(store.messages.length, 1);
    expect(store.messages.first.body, 'on it');
    expect(store.messages.first.author.isAgent, isTrue);
  });

  test('an amended task shows what it asks for now', () async {
    final client = FakeClient();
    final store = RoomStore(client);

    client.emit(
      event({
        'event': 'task_created',
        'task': {
          'id': 'task_1',
          'title': 'fix the squad count',
          'brief': 'b',
          'state': 'in_review',
          'priority': 'urgent',
          'revision': 0,
          'acceptance_criteria': ['a join-level test covers the last resort'],
        },
      }),
    );
    client.emit(
      event({
        'event': 'task_amended',
        'task': 'task_1',
        'title': 'fix the squad count',
        'by': 'bot_lead',
        'acceptance_criteria': ['late parties are seated', 'a direct test covers the last resort'],
        'reason': 'the join-level test is waived',
      }, seq: 2),
    );
    await Future<void>.delayed(Duration.zero);

    expect(store.tasks.first.acceptanceCriteria, ['late parties are seated', 'a direct test covers the last resort']);
    expect(store.tasks.first.brief, 'b', reason: 'the brief stays when only the criteria change');
  });

  test('a parked task drops its holder, and park and resume reach the room', () async {
    final client = FakeClient();
    final store = RoomStore(client);

    client.emit(
      event({
        'event': 'task_created',
        'task': {
          'id': 'task_1',
          'title': 'launch readiness',
          'brief': 'b',
          'state': 'in_progress',
          'priority': 'normal',
          'revision': 0,
          'assignee': 'bot_b',
        },
      }),
    );
    client.emit(
      event({
        'event': 'task_transitioned',
        'task': 'task_1',
        'from': 'in_progress',
        'to': 'parked',
        'transition': {'transition': 'park', 'reason': 'after the launch'},
      }, seq: 2),
    );
    await Future<void>.delayed(Duration.zero);

    final task = store.tasks.first;
    expect(task.state, TaskState.parked);
    expect(task.state.label, 'parked');
    expect(task.assignee, isNull, reason: 'nobody holds parked work');

    store.resumeTask('task_1');
    store.parkTask('task_1', reason: 'not yet');
    expect(client.sent, [
      {'type': 'resume_task', 'task': 'task_1'},
      {'type': 'park_task', 'task': 'task_1', 'reason': 'not yet'},
    ]);
  });

  test('pending removals arrive with the digest and are reported back', () async {
    final client = FakeClient();
    final store = RoomStore(client);
    client.emit({
      'type': 'digest',
      'digest': {
        'pending_removals': [
          {'agent': 'bot_a', 'handle': 'deepseek-builder', 'copy': 'room tools', 'count': 1549},
          {'agent': 'bot_b', 'handle': 'mira', 'copy': 'its own CLI', 'count': 2},
        ],
      },
    });
    await Future<void>.delayed(Duration.zero);
    expect(store.digest.pendingRemovalTotal, 1551);
    expect(store.digest.pendingRemovals.first.handle, 'deepseek-builder');

    store.reportPendingRemovals('bot_a', 'room tools', 0);
    expect(client.sent.last, {
      'type': 'report_pending_removals',
      'agent': 'bot_a',
      'copy': 'room tools',
      'count': 0,
    });
  });

  test('a task transition updates state and ownership', () async {
    final client = FakeClient();
    final store = RoomStore(client);

    client.emit(
      event({
        'event': 'task_created',
        'task': {
          'id': 'task_1',
          'title': 't',
          'brief': 'b',
          'state': 'ready',
          'priority': 'normal',
          'revision': 0,
        },
      }),
    );
    client.emit(
      event({
        'event': 'task_transitioned',
        'task': 'task_1',
        'from': 'ready',
        'to': 'claimed',
        'transition': {'transition': 'claim', 'by': 'bot_b'},
        'actor': 'bot_b',
      }, seq: 2),
    );
    await Future<void>.delayed(Duration.zero);

    final task = store.tasks.first;
    expect(task.state, TaskState.claimed);
    expect(task.assignee, 'bot_b');
  });

  for (final kind in ['delegate', 'assist', 'review', 'escalate']) {
    test('accepted $kind handoff folds ownership live and on replay', () async {
      final client = FakeClient();
      final store = RoomStore(client);
      addTearDown(store.dispose);
      final frames = [
        event({
          'event': 'task_created',
          'task': {
            'id': 'task_1',
            'title': 'Finish the work',
            'state': 'in_progress',
            'assignee': 'bot_a',
          },
        }),
        event({
          'event': 'handoff_offered',
          'handoff': {
            'id': 'handoff_1',
            'task': 'task_1',
            'from': 'bot_a',
            'kind': kind,
            'state': 'offered',
          },
        }, seq: 2),
        event({
          'event': 'handoff_transitioned',
          'handoff': 'handoff_1',
          'from': 'offered',
          'to': 'accepted',
          'transition': {'transition': 'accept', 'by': 'bot_b'},
        }, seq: 3),
      ];
      for (final frame in frames) {
        client.emit(frame);
      }
      await Future<void>.delayed(Duration.zero);

      void expectAccepted(RoomStore view) {
        expect(view.handoffs.single.state, 'accepted');
        expect(view.handoffs.single.acceptedBy, 'bot_b');
        expect(view.tasks.single.assignee, kind == 'delegate' ? 'bot_b' : 'bot_a');
      }

      expectAccepted(store);
      final restored = RoomStore(FakeClient());
      addTearDown(restored.dispose);
      restored.restore([
        {
          'type': 'snapshot',
          'head': 3,
          'tasks': [
            {
              'id': 'task_1',
              'title': 'Finish the work',
              'state': 'in_progress',
              'assignee': kind == 'delegate' ? 'bot_b' : 'bot_a',
            },
          ],
        },
        ...frames,
      ]);
      expectAccepted(restored);

      // A duplicate acceptance must not undo a later release during reconnect.
      final release = event({
        'event': 'task_transitioned',
        'task': 'task_1',
        'to': 'ready',
        'transition': {'transition': 'release'},
      }, seq: 4);
      store.restore([release, frames.last]);
      expect(store.tasks.single.assignee, isNull);
      expect(store.handoffs.single.acceptedBy, 'bot_b');
    });
  }

  test('request_changes bumps the revision counter', () async {
    final client = FakeClient();
    final store = RoomStore(client);
    client.emit(
      event({
        'event': 'task_created',
        'task': {
          'id': 'task_1',
          'title': 't',
          'brief': 'b',
          'state': 'in_review',
          'priority': 'normal',
          'revision': 0,
        },
      }),
    );
    client.emit(
      event({
        'event': 'task_transitioned',
        'task': 'task_1',
        'from': 'in_review',
        'to': 'in_progress',
        'transition': {
          'transition': 'request_changes',
          'by': 'bot_qa',
          'reason': 'nope',
        },
      }, seq: 2),
    );
    await Future<void>.delayed(Duration.zero);
    expect(store.tasks.first.revision, 1);
  });

  test('an unknown task state degrades gracefully rather than throwing', () {
    final task = Task.fromJson({
      'id': 'task_1',
      'title': 't',
      'brief': '',
      'state': 'teleporting',
    });
    expect(task.state, TaskState.unknown);
  });

  test('agent status changes are applied live', () async {
    final client = FakeClient();
    final store = RoomStore(client);
    client.emit({
      'type': 'snapshot',
      'room': {
        'id': 'r',
        'spec': {'name': 'x', 'goal': 'y'},
        'phase': 'running',
      },
      'roster': [
        {
          'id': 'bot_a',
          'handle': 'w1',
          'display_name': 'W',
          'roles': ['writer'],
          'status': 'idle',
        },
      ],
      'tasks': [],
      'head': 0,
    });
    client.emit(
      event({
        'event': 'agent_status_changed',
        'agent': 'bot_a',
        'from': 'idle',
        'to': 'working',
      }),
    );
    await Future<void>.delayed(Duration.zero);
    expect(store.agents.first.status, AgentStatus.working);
  });

  test('message view is stable until transcript content changes', () async {
    final client = FakeClient();
    final store = RoomStore(client);
    addTearDown(store.dispose);
    final empty = store.messages;
    expect(identical(store.messages, empty), isTrue);

    store.addPending('Alex', 'pending note');
    final pending = store.messages;
    expect(identical(pending, store.messages), isTrue);
    expect(pending.single.body, 'pending note');

    client.emit({
      'type': 'snapshot',
      'room': {
        'id': 'room_1',
        'spec': {'name': 'Room', 'goal': ''},
      },
      'roster': [
        {
          'id': 'bot_a',
          'handle': 'iris',
          'display_name': 'Iris',
          'roles': [],
          'status': 'idle',
        },
      ],
      'tasks': [],
      'head': 0,
    });
    await Future<void>.delayed(Duration.zero);
    final reset = store.messages;
    expect(identical(reset, pending), isFalse);
    expect(reset.single.body, 'pending note');
    final awaitingEcho = reset;

    client.emit(
      event({
        'event': 'message_posted',
        'message': {
          'id': 'msg_1',
          'author': {'kind': 'human', 'name': 'Alex'},
          'body': 'pending note',
          'intent': 'ask',
          'sent_at': 1,
        },
      }),
    );
    await Future<void>.delayed(Duration.zero);
    final confirmed = store.messages;
    expect(identical(confirmed, store.messages), isTrue);
    expect(identical(confirmed, awaitingEcho), isFalse);
    expect(identical(confirmed, reset), isFalse);
    expect(confirmed.single.id, 'msg_1');

    var notifications = 0;
    store.addListener(() => notifications++);
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'text_delta', 'text': 'activity only'},
    });
    await Future<void>.delayed(Duration.zero);
    expect(notifications, 1);
    expect(identical(store.messages, confirmed), isTrue);
  });

  test('process activity is bounded and token deltas do not fan out', () async {
    final client = FakeClient();
    final store = RoomStore(client);
    addTearDown(store.dispose);
    client.emit({
      'type': 'snapshot',
      'roster': [
        {
          'id': 'bot_a',
          'handle': 'iris',
          'display_name': 'Iris',
          'roles': [],
          'status': 'idle',
        },
      ],
      'tasks': [],
    });
    await Future<void>.delayed(Duration.zero);
    var notifications = 0;
    store.addListener(() => notifications++);

    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'thinking'},
    });
    await Future<void>.delayed(Duration.zero);
    expect(store.activity.single.streaming, isTrue);
    expect(notifications, 1);
    for (var i = 0; i < 100; i++) {
      client.emit({
        'type': 'stream',
        'agent': 'bot_a',
        'event': {'type': 'text_delta', 'text': 'token $i'},
      });
    }
    await Future<void>.delayed(Duration.zero);
    expect(notifications, 1, reason: 'token frames must not rebuild the UI');

    for (var i = 0; i < 20; i++) {
      client.emit({
        'type': 'stream',
        'agent': 'bot_a',
        'event': {
          'type': 'tool_use_start',
          'id': 'tool_$i',
          'name': 'workspace_read',
        },
      });
      client.emit({
        'type': 'stream',
        'agent': 'bot_a',
        'event': {'type': 'tool_use_end', 'id': 'tool_$i', 'succeeded': true},
      });
    }
    await Future<void>.delayed(const Duration(milliseconds: 24));
    final steps = store.activity.single.steps;
    expect(steps, hasLength(12));
    expect(steps.first.id, 'tool_8');
    expect(steps.last.id, 'tool_19');
    expect(
      steps.every((step) => !step.running && step.succeeded == true),
      isTrue,
    );
    expect(store.activity.single.activeStep, isNull);
    expect(notifications, greaterThan(1));

    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {
        'type': 'tool_use_start',
        'id': 'tool_unconfirmed',
        'name': 'workspace_write',
      },
    });
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'done'},
    });
    await Future<void>.delayed(Duration.zero);
    final interrupted = store.activity.single.steps.last;
    expect(interrupted.id, 'tool_unconfirmed');
    expect(interrupted.running, isFalse);
    expect(interrupted.succeeded, isNull);
  });

  test('large history restore batches notifications into one update', () {
    final client = FakeClient();
    final store = RoomStore(client);
    addTearDown(store.dispose);
    var notifications = 0;
    store.addListener(() => notifications++);
    store.restore([
      {
        'type': 'snapshot',
        'head': 500,
        'room': {
          'id': 'room_1',
          'spec': {'name': 'History', 'goal': 'Restore history'},
          'phase': 'running',
        },
        'roster': [],
        'tasks': [],
      },
      for (var i = 1; i <= 500; i++)
        event({
          'event': 'message_posted',
          'message': {
            'id': 'msg_$i',
            'room': 'room_1',
            'author': {'kind': 'human', 'name': 'You'},
            'audience': {'kind': 'everyone'},
            'intent': 'say',
            'body': 'Restored line $i',
            'sent_at': i,
          },
        }, seq: i),
    ]);
    expect(store.messages, hasLength(500));
    expect(store.messages.last.body, 'Restored line 500');
    expect(notifications, 1);
  });

  test('large reconnect replay coalesces transcript notifications', () async {
    final client = FakeClient();
    final store = RoomStore(client);
    addTearDown(store.dispose);
    client.emit({'type': 'snapshot', 'head': 500, 'roster': [], 'tasks': []});
    await Future<void>.delayed(Duration.zero);
    var notifications = 0;
    store.addListener(() => notifications++);
    for (var i = 1; i <= 500; i++) {
      client.emit(
        event({
          'event': 'message_posted',
          'message': {
            'id': 'replay_$i',
            'room': 'room_1',
            'author': {'kind': 'agent', 'agent': 'bot_a'},
            'audience': {'kind': 'everyone'},
            'intent': 'say',
            'body': 'Replay line $i',
            'sent_at': i,
          },
        }, seq: i),
      );
    }
    await Future<void>.delayed(const Duration(milliseconds: 24));
    expect(store.messages, hasLength(500));
    expect(notifications, 1);
  });

  test('parallel model activity remains independent per agent', () async {
    final client = FakeClient();
    final store = RoomStore(client);
    addTearDown(store.dispose);
    client.emit({
      'type': 'snapshot',
      'roster': [
        for (final id in ['bot_a', 'bot_b'])
          {
            'id': id,
            'handle': id,
            'display_name': id,
            'roles': [],
            'status': 'working',
          },
      ],
      'tasks': [],
    });
    await Future<void>.delayed(Duration.zero);
    var notifications = 0;
    store.addListener(() => notifications++);

    void stream(String agent, Map<String, Object?> event) =>
        client.emit({'type': 'stream', 'agent': agent, 'event': event});

    stream('bot_a', {'type': 'thinking'});
    stream('bot_b', {'type': 'thinking'});
    stream('bot_a', {'type': 'retry', 'attempt': 1, 'max_retries': 3});
    stream('bot_b', {'type': 'thinking'});
    stream('bot_b', {
      'type': 'tool_use_start',
      'id': 'parallel-tool',
      'name': 'workspace_read',
    });
    stream('bot_b', {
      'type': 'tool_use_end',
      'id': 'parallel-tool',
      'succeeded': true,
    });
    await Future<void>.delayed(const Duration(milliseconds: 24));

    final activity = {for (final item in store.activity) item.agent.id: item};
    expect(activity['bot_a']!.streaming, isTrue);
    expect(activity['bot_a']!.retryStatus, 'Reconnecting (1/3)');
    expect(activity['bot_b']!.streaming, isTrue);
    expect(activity['bot_b']!.steps.single.succeeded, isTrue);
    expect(notifications, 4, reason: 'token-independent state changes only');

    stream('bot_a', {'type': 'done'});
    await Future<void>.delayed(Duration.zero);
    expect(activity['bot_a']!.streaming, isFalse);
    expect(activity['bot_a']!.retryStatus, isNull);
    expect(activity['bot_b']!.streaming, isTrue);
    expect(notifications, 5);
  });
}
