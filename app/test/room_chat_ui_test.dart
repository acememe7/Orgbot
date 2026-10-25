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
  testWidgets('a retry shows thinking and retains the original failure time', (
    tester,
  ) async {
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
    await tester.pump();
    expect(store.failing, isEmpty);
    expect(find.text('@iris is thinking'), findsOneWidget);
    expect(find.text('thinking'), findsOneWidget);
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
    expect(find.text('@iris is thinking'), findsNothing);
    expect(tester.takeException(), isNull);
    // A real new error must still stop thinking and appear, even when the bot
    // holds a task with status "working".
    client.emit(
      event({
        'event': 'agent_errored',
        'agent': 'bot_a',
        'error': 'new network failure',
        'attempt': 2,
      }, seq: 2),
    );
    await tester.pump();
    expect(store.failing.single.lastError, 'new network failure');
    expect(find.text('@iris is thinking'), findsNothing);
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
}
