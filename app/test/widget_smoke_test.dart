/// The room screen must render from a store without a live connection.
library;

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/net/room_client.dart';
import 'package:orgbot_app/state/room_store.dart';
import 'package:orgbot_app/ui/room_screen.dart';

class FakeClient implements RoomClient {
  final _messages = StreamController<Map<String, Object?>>.broadcast();
  final _state = StreamController<LinkState>.broadcast();
  void emit(Map<String, Object?> m) => _messages.add(m);
  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;
  @override
  Stream<LinkState> get connectionState => _state.stream;
  @override
  LinkState get current => LinkState.connected;
  @override
  void send(Map<String, Object?> m) {}
  @override
  void sendMessage(String a, String b, {String? taskId, String? replyTo}) {}
  @override
  void answerBot(String t, String a) {}
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

void main() {
  testWidgets('the room screen shows the goal, a bot, and a task', (tester) async {
    // A wide surface so the three-pane layout is exercised.
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);

    final client = FakeClient();
    final store = RoomStore(client);
    client.emit({
      'type': 'snapshot',
      'room': {
        'id': 'r',
        'spec': {'name': 'Release Room', 'goal': 'ship the notes', 'limits': {'max_total_turns': 100}},
        'phase': 'running',
        'turns_used': 0,
      },
      'roster': [
        {'id': 'bot_a', 'handle': 'lead', 'display_name': 'Lead', 'roles': ['coordinator'], 'status': 'idle'},
      ],
      'tasks': [
        {'id': 'task_1', 'title': 'Write the notes', 'brief': 'do it', 'state': 'ready', 'priority': 'high', 'revision': 0},
      ],
      'head': 0,
    });
    // Fold the snapshot in. The store subscribes to the client's streams in
    // its constructor, and delivery happens on a microtask: a bare
    // `Future.delayed` await would schedule a fake timer that never fires in
    // the testWidgets fake-async zone, so pump explicitly instead.
    await tester.pump();

    await tester.pumpWidget(MaterialApp(
      home: RoomScreen(store: store, client: client, displayName: 'tester'),
    ));
    await tester.pump();

    expect(find.text('Release Room'), findsOneWidget);
    expect(find.text('ship the notes'), findsOneWidget);
    expect(find.text('@lead'), findsOneWidget);
    expect(find.text('Write the notes'), findsOneWidget);
    expect(find.text('Message or @mention a teammate…'), findsOneWidget);
  });
}
