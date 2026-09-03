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
  final _messages =
      StreamController<Map<String, Object?>>.broadcast(sync: true);
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
  void sendMessage(String a, String b, {String? taskId}) =>
      sent.add({'author': a, 'body': b});
  @override
  void answerBot(String t, String a) {}
  @override
  void pause({String reason = ''}) {}
  @override
  void resume() {}
  @override
  void resync() {}
  @override
  Future<void> connect(String url) async {}
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
      {'id': 'bot_a', 'handle': 'iris', 'display_name': 'Iris', 'roles': ['engineer'], 'status': 'working'},
      {'id': 'bot_b', 'handle': 'quill', 'display_name': 'Quill', 'roles': ['reviewer'], 'status': 'idle'},
    ],
    'tasks': [
      {'id': 'task_1', 'title': 'Write the notes', 'brief': 'b', 'state': 'in_progress', 'priority': 'high', 'assignee': 'bot_a', 'revision': 0},
    ],
    'head': 0,
  });
  return store;
}

void main() {
  testWidgets('the composer sends what you type', (tester) async {
    final controller = TextEditingController();
    var sent = '';
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: Composer(
            controller: controller, onSend: () => sent = controller.text),
      ),
    ));

    expect(find.byType(TextField), findsOneWidget);
    await tester.enterText(find.byType(TextField), 'how is it going?');
    await tester.tap(find.byIcon(Icons.arrow_upward));
    await tester.pump();
    expect(sent, 'how is it going?');
  });

  testWidgets('the transcript shows who said what', (tester) async {
    final client = FakeClient();
    final store = seeded(client);
    client.emit(event({
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
    }));

    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
          body: Transcript(store: store, controller: ScrollController())),
    ));
    await tester.pump();

    expect(find.text('@iris'), findsOneWidget);
    expect(find.text('on it'), findsOneWidget);
  });

  /// The point of the rail: work is visible without reading the chat.
  testWidgets('the activity rail shows each bot and what it is doing',
      (tester) async {
    tester.view.physicalSize = const Size(500, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);

    final client = FakeClient();
    final store = seeded(client);
    client.emit(event({
      'event': 'task_transitioned',
      'task': 'task_1',
      'from': 'claimed',
      'to': 'in_progress',
      'transition': {'transition': 'start'},
      'actor': 'bot_a',
    }, seq: 2));

    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: ActivityRail(store: store))));
    await tester.pump();

    expect(find.text('Activity'), findsOneWidget);
    expect(find.text('@iris'), findsOneWidget);
    expect(find.text('@quill'), findsOneWidget);
    expect(find.textContaining('is working on'), findsOneWidget,
        reason: 'the rail must say what the bot is actually doing');
    expect(find.textContaining('delivered'), findsOneWidget,
        reason: 'board progress belongs in the rail footer');
  });

  testWidgets('the board strip lists tasks with their owner', (tester) async {
    final client = FakeClient();
    final store = seeded(client);
    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: BoardStrip(store: store))));
    await tester.pump();

    expect(find.text('Write the notes'), findsOneWidget);
    expect(find.text('@iris'), findsOneWidget);
    expect(find.text('in progress'), findsOneWidget);
  });
}
