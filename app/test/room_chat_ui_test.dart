// The room's pieces: a transcript, a composer, and the activity rail that
// shows what each bot is doing.
//
// These drive the widgets against a store rather than the whole screen: the
// screen starts a real engine process, which a widget test should not do.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/net/room_client.dart';
import 'package:orgbot_app/state/room_store.dart';
import 'package:orgbot_app/ui/room_widgets.dart';

class FakeClient implements RoomClient {
  final _messages = StreamController<Map<String, Object?>>.broadcast(
    sync: true,
  );
  final _state = StreamController<LinkState>.broadcast(sync: true);
  final sent = <Map<String, Object?>>[];
  void emit(Map<String, Object?> m) => _messages.add(m);
  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;
  @override
  Stream<LinkState> get connectionState => _state.stream;
  @override
  LinkState get current => LinkState.connected;
  @override
  void send(Map<String, Object?> m) => sent.add(m);
  @override
  void sendMessage(String a, String b, {String? taskId, String? replyTo}) =>
      sent.add({'author': a, 'body': b});
  @override
  void answerBot(String t, String a) =>
      sent.add({'type': 'answer_bot', 'task': t, 'answer': a});
  @override
  void answerDecision(String decision, String chosen, {String note = ''}) {}

  @override
  void pause({String reason = ''}) {}
  @override
  void stopNow({String reason = ''}) {}
  @override
  void resume() {}
  @override
  void resync() {}
  @override
  Future<void> connect(String url, {String? token}) async {}
  @override
  Future<void> close() async {}
}

Map<String, Object?> event(Map<String, Object?> inner, {int seq = 1}) => {
  'type': 'event',
  'envelope': {'id': 'ev', 'seq': seq, 'room': 'r', 'at': 0, 'event': inner},
};

RoomStore seeded(FakeClient client) {
  final store = RoomStore(client);
  client.emit({
    'type': 'snapshot',
    'room': {
      'id': 'r',
      'spec': {'name': 'Release', 'goal': 'ship it'},
      'phase': 'running',
    },
    'roster': [
      {
        'id': 'bot_a',
        'handle': 'iris',
        'display_name': 'Iris',
        'roles': ['engineer'],
        'status': 'working',
      },
      {
        'id': 'bot_b',
        'handle': 'quill',
        'display_name': 'Quill',
        'roles': ['reviewer'],
        'status': 'idle',
      },
    ],
    'tasks': [
      {
        'id': 'task_1',
        'title': 'Write the notes',
        'brief': 'b',
        'state': 'in_progress',
        'priority': 'high',
        'assignee': 'bot_a',
        'revision': 0,
      },
    ],
    'head': 0,
  });
  return store;
}

void main() {
  testWidgets('a retry shows working and retains the original failure time', (
    tester,
  ) async {
    // The room says who is working; the server resends it as turns start and
    // end.
    Map<String, Object?> doing(String iris) => {
      'type': 'digest',
      'digest': {
        'utilization': {
          'bots': [
            {'agent': 'bot_a', 'handle': 'iris', 'doing': iris, 'holding': 1, 'free': false},
            {'agent': 'bot_b', 'handle': 'quill', 'doing': 'idle', 'holding': 0, 'free': true},
          ],
        },
      },
    };
    final client = FakeClient();
    final store = seeded(client);
    addTearDown(store.dispose);
    final failureTime = DateTime.now().subtract(const Duration(minutes: 10));
    final failed = event({
      'event': 'agent_errored',
      'agent': 'bot_a',
      'error': 'error decoding response body',
      'attempt': 1,
    });
    (failed['envelope'] as Map)['at'] = failureTime.millisecondsSinceEpoch;
    client.emit(failed);
    final scroll = ScrollController();
    addTearDown(scroll.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ListenableBuilder(
            listenable: store,
            builder: (_, _) => Column(
              children: [
                Expanded(
                  child: Transcript(store: store, controller: scroll),
                ),
                Expanded(child: ActivityRail(store: store)),
              ],
            ),
          ),
        ),
      ),
    );
    expect(find.text('@iris — previous turn failed'), findsOneWidget);
    expect(find.text('Previous turn failed · 10m ago'), findsOneWidget);
    expect(
      store.activity.first.lastErrorAt?.millisecondsSinceEpoch,
      failureTime.millisecondsSinceEpoch,
    );
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'text_delta', 'text': 'Trying again'},
    });
    client.emit(doing('working'));
    await tester.pump();
    expect(store.failing, isEmpty);
    expect(find.text('@iris is working'), findsOneWidget);
    expect(find.text('working'), findsOneWidget);
    expect(find.text('@iris — previous turn failed'), findsNothing);
    expect(find.text('Previous turn failed · 10m ago'), findsOneWidget);
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'retry', 'attempt': 1, 'max_retries': 2},
    });
    await tester.pump();
    expect(find.text('@iris Reconnecting (1/2)'), findsOneWidget);
    expect(find.text('Reconnecting (1/2)'), findsOneWidget);
    expect(find.text('@iris is working'), findsNothing);
    expect(tester.takeException(), isNull);
    // A real new error ends the turn and must appear, even when the bot holds
    // a task with status "working".
    client.emit(
      event({
        'event': 'agent_errored',
        'agent': 'bot_a',
        'error': 'new network failure',
        'attempt': 2,
      }, seq: 2),
    );
    client.emit(doing('between turns'));
    await tester.pump();
    expect(store.failing.single.lastError, 'new network failure');
    expect(find.text('@iris is working'), findsNothing);
    expect(find.text('@iris — previous turn failed'), findsOneWidget);
    client.emit(
      event({
        'event': 'message_posted',
        'message': {
          'id': 'm_done',
          'room': 'r',
          'author': {'kind': 'agent', 'agent': 'bot_a'},
          'audience': {'kind': 'everyone'},
          'intent': 'say',
          'body': 'Done',
          'sent_at': 0,
        },
      }, seq: 3),
    );
    await tester.pump();
    expect(store.failing, isEmpty);
    expect(find.textContaining('previous turn failed'), findsNothing);
  });

  testWidgets('the composer sends what you type', (tester) async {
    final controller = TextEditingController();
    var sent = '';
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Composer(
            controller: controller,
            onSend: () => sent = controller.text,
          ),
        ),
      ),
    );

    expect(find.byType(TextField), findsOneWidget);
    expect(
      tester.widget<FilledButton>(find.byType(FilledButton)).onPressed,
      isNull,
    );
    await tester.enterText(find.byType(TextField), 'how is it going?');
    await tester.pump();
    await tester.tap(find.byTooltip('Send message'));
    await tester.pump();
    expect(sent, 'how is it going?');
  });

  testWidgets('the transcript shows who said what', (tester) async {
    final client = FakeClient();
    final store = seeded(client);
    client.emit(
      event({
        'event': 'message_posted',
        'message': {
          'id': 'm1',
          'room': 'r',
          'author': {'kind': 'agent', 'agent': 'bot_a'},
          'audience': {'kind': 'everyone'},
          'intent': 'say',
          'body': 'on it',
          'sent_at': 0,
        },
      }),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Transcript(store: store, controller: ScrollController()),
        ),
      ),
    );
    await tester.pump();

    expect(find.text('Iris'), findsOneWidget);
    expect(find.text('on it'), findsOneWidget);
  });

  /// The point of the rail: work is visible without reading the chat.
  testWidgets('the activity rail shows each bot and what it is doing', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(500, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);

    final client = FakeClient();
    final store = seeded(client);
    client.emit(
      event({
        'event': 'task_transitioned',
        'task': 'task_1',
        'from': 'claimed',
        'to': 'in_progress',
        'transition': {'transition': 'start'},
        'actor': 'bot_a',
      }, seq: 2),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: ActivityRail(store: store)),
      ),
    );
    await tester.pump();

    expect(find.text('Activity'), findsOneWidget);
    expect(find.text('Iris'), findsOneWidget);
    expect(find.text('Quill'), findsOneWidget);
    expect(
      find.textContaining('is working on'),
      findsOneWidget,
      reason: 'the rail must say what the bot is actually doing',
    );
    expect(
      find.textContaining('delivered'),
      findsOneWidget,
      reason: 'board progress belongs in the rail footer',
    );
  });

  // From benchmark 2: the owner asked to stop one bot running long turns
  // with no task, and the room could only stop every bot or none.
  testWidgets('the owner can pause one bot from the activity rail', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(500, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);

    final client = FakeClient();
    final store = seeded(client);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: ActivityRail(store: store)),
      ),
    );
    await tester.pump();
    await tester.tap(find.byTooltip('Bot actions').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Pause @iris and free its work'));
    await tester.pumpAndSettle();
    expect(client.sent.last, {'type': 'pause_bot', 'agent': 'bot_a'});

    await tester.tap(find.byTooltip('Bot actions').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove @iris'));
    await tester.pumpAndSettle();
    expect(find.text('Remove @iris from this run?'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilledButton, 'Remove @iris'));
    await tester.pumpAndSettle();
    expect(client.sent.last, {'type': 'remove_bot', 'agent': 'bot_a'});
  });

  // From benchmark 2: nothing in the app could say where a 90-minute turn
  // had gone, and a bot the room benched must say why.
  testWidgets('each bot row says where its time went and why it is benched', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(500, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);

    final client = FakeClient();
    final store = seeded(client);
    client.emit({
      'type': 'digest',
      'digest': {
        'utilization': {
          'bots': [
            {
              'agent': 'bot_a',
              'handle': 'iris',
              'doing': 'benched',
              'holding': 0,
              'free': false,
              'time': {
                'turns': 3,
                'turn_ms': 111 * 60000,
                'longest_turn_ms': 94 * 60000,
                'current_turn_ms': 0,
                'workspace_ms': 86 * 60000,
                'workspace_calls': 128,
              },
            },
          ],
        },
      },
    });
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: ActivityRail(store: store)),
      ),
    );
    await tester.pump();
    expect(
      find.text(
        '1 h 51 min in 3 turns, longest 1 h 34 min · 1 h 26 min in 128 workspace calls',
      ),
      findsOneWidget,
    );
    expect(find.text('benched: long turn, no task'), findsOneWidget);
    await tester.tap(find.byTooltip('Bot actions').first);
    await tester.pumpAndSettle();
    expect(find.text('Resume @iris'), findsOneWidget);
  });

  // From speed test 1: the owner wrote "@grok-quick ... I'm asking the room",
  // and only @grok-quick heard it.
  test('the composer says who a message will reach', () {
    const handles = ['grok-quick', 'opus'];
    expect(composerReach('just a note', handles), isNull);
    expect(
      composerReach('@grok-quick submit it, @opus split it', handles),
      'Goes to @grok-quick, @opus. The coordinators see a copy; @all reaches everyone.',
    );
    expect(composerReach('@all stop', handles), 'Goes to everyone in the room.');
    expect(composerReach('@nobody hi', handles), isNull);
  });

  testWidgets('the activity rail exposes a collapsible process timeline', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(500, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);

    final client = FakeClient();
    final store = seeded(client);
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {
        'type': 'tool_use_start',
        'id': 'process-1',
        'name': 'Run command',
      },
    });
    client.emit({
      'type': 'stream',
      'agent': 'bot_a',
      'event': {'type': 'tool_use_end', 'id': 'process-1', 'succeeded': true},
    });
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: ActivityRail(store: store)),
      ),
    );
    await tester.pump();
    expect(find.text('Process · 1 recent steps'), findsOneWidget);
    await tester.tap(find.text('Process · 1 recent steps'));
    await tester.pumpAndSettle();
    expect(find.text('Run command'), findsOneWidget);
    expect(find.text('done'), findsOneWidget);
  });

  testWidgets('the board strip lists tasks with their owner', (tester) async {
    final client = FakeClient();
    final store = seeded(client);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: BoardStrip(store: store)),
      ),
    );
    await tester.pump();

    expect(find.text('Write the notes'), findsOneWidget);
    expect(find.text('@iris'), findsOneWidget);
    expect(find.text('in progress'), findsOneWidget);

    client.emit(
      event({
        'event': 'task_transitioned',
        'task': 'task_1',
        'from': 'in_progress',
        'to': 'blocked',
        'actor': 'bot_a',
        'transition': {
          'transition': 'block',
          'reason': {
            'kind': 'awaiting_human',
            'prompt': 'Reconnect SSH so I can verify the sources.',
          },
        },
      }, seq: 2),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: BoardStrip(store: store)),
      ),
    );
    await tester.pump();
    expect(
      find.text('Reconnect SSH so I can verify the sources.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Reply to unblock'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byType(TextField),
      'SSH is connected. Verify the sources.',
    );
    await tester.tap(find.text('Send reply'));
    await tester.pumpAndSettle();
    expect(client.sent.last, {
      'type': 'answer_bot',
      'task': 'task_1',
      'answer': 'SSH is connected. Verify the sources.',
    });
  });

  testWidgets('task cards fold to one line and open with a tap', (
    tester,
  ) async {
    // From the live room: 48 tasks of up to five lines each.
    final client = FakeClient();
    final store = seeded(client);
    client.emit(
      event({
        'event': 'task_transitioned',
        'task': 'task_1',
        'from': 'in_progress',
        'to': 'blocked',
        'actor': 'bot_a',
        'transition': {
          'transition': 'block',
          'reason': {
            'kind': 'awaiting_handoff',
            'prompt': 'Waiting on Sol for the haze diagnosis.',
          },
        },
      }, seq: 2),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: BoardStrip(store: store)),
      ),
    );
    await tester.pump();
    // A blocked task lives under Waiting.
    await tester.tap(find.byKey(const ValueKey('tasks-tab-waiting')));
    await tester.pumpAndSettle();
    expect(find.text('Write the notes'), findsOneWidget);
    expect(
      find.text('@iris'),
      findsOneWidget,
      reason: 'the owner still shows when folded',
    );
    expect(find.text('Waiting on Sol for the haze diagnosis.'), findsNothing);
    await tester.tap(find.byKey(const ValueKey('task-fold-task_1')));
    await tester.pump();
    expect(find.text('Waiting on Sol for the haze diagnosis.'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('tasks-fold-all')));
    await tester.pump();
    expect(find.text('Waiting on Sol for the haze diagnosis.'), findsNothing);
    expect(find.text('Expand all'), findsOneWidget);
  });

  testWidgets('the owner parks a task from the board and resumes it', (
    tester,
  ) async {
    final client = FakeClient();
    final store = seeded(client);
    Future<void> show() async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: BoardStrip(store: store)),
        ),
      );
      // The tab change fades; wait for it.
      await tester.pumpAndSettle();
    }

    await show();
    await tester.tap(find.byTooltip('Task actions'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Park until you resume it'));
    await tester.pumpAndSettle();
    expect(find.textContaining('until you resume it'), findsOneWidget);
    await tester.enterText(find.byType(TextField), 'after the launch');
    await tester.tap(find.text('Park task'));
    await tester.pumpAndSettle();
    expect(client.sent.last, {
      'type': 'park_task',
      'task': 'task_1',
      'reason': 'after the launch',
    });

    client.emit(
      event({
        'event': 'task_transitioned',
        'task': 'task_1',
        'from': 'in_progress',
        'to': 'parked',
        'transition': {'transition': 'park', 'reason': 'after the launch'},
      }, seq: 2),
    );
    await show();
    expect(find.text('parked'), findsOneWidget);
    expect(
      find.text('@iris'),
      findsNothing,
      reason: 'nobody holds parked work',
    );
    await tester.tap(find.byTooltip('Task actions'));
    await tester.pumpAndSettle();
    expect(
      find.text('Park until you resume it'),
      findsNothing,
      reason: 'parked work offers Resume instead',
    );
    await tester.tap(find.text('Resume task').last);
    await tester.pumpAndSettle();
    expect(client.sent.last, {'type': 'resume_task', 'task': 'task_1'});
  });
}
