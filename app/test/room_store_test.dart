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
  void sendMessage(String author, String body, {String? taskId}) =>
      sent.add({'type': 'send_message', 'author': author, 'body': body});
  @override
  void answerBot(String taskId, String answer) {}
  @override
  void answerDecision(String decision, String chosen, {String note = ''}) =>
      answered.add((decision: decision, chosen: chosen));
  final answered = <({String decision, String chosen})>[];
  @override
  void pause({String reason = ''}) => sent.add({'type': 'pause'});
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
}
