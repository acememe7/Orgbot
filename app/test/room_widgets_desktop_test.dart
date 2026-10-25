import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/net/room_client.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/state/room_store.dart';
import 'package:orgbot_app/ui/room_widgets.dart';

class _Client implements RoomClient {
  final _messages = StreamController<Map<String, Object?>>.broadcast(
    sync: true,
  );
  final _state = StreamController<LinkState>.broadcast(sync: true);

  void emit(Map<String, Object?> message) => _messages.add(message);

  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;
  @override
  Stream<LinkState> get connectionState => _state.stream;
  @override
  LinkState get current => LinkState.connected;
  @override
  void send(Map<String, Object?> message) {}
  @override
  void sendMessage(
    String author,
    String body, {
    String? taskId,
    String? replyTo,
  }) {}
  @override
  void answerBot(String task, String answer) {}
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

RoomStore _store(
  _Client client, {
  List<Map<String, Object?>> messages = const [],
}) {
  final store = RoomStore(client);
  client.emit({
    'type': 'snapshot',
    'room': {
      'id': 'room_desktop',
      'spec': {'name': 'Release', 'goal': 'ship the desktop room'},
      'phase': 'running',
    },
    'roster': [
      {
        'id': 'bot_iris',
        'handle': 'iris',
        'display_name': 'Iris',
        'roles': ['engineer'],
        'status': 'idle',
      },
    ],
    'tasks': [],
    'head': 0,
  });
  var sequence = 0;
  for (final message in messages) {
    sequence++;
    client.emit({
      'type': 'event',
      'envelope': {
        'id': 'event_$sequence',
        'seq': sequence,
        'room': 'room_desktop',
        'at': 0,
        'event': {'event': 'message_posted', 'message': message},
      },
    });
  }
  return store;
}

Map<String, Object?> _message(
  String id,
  String body, {
  String kind = 'human',
  String? replyTo,
  String intent = 'say',
}) => {
  'id': id,
  'room': 'room_desktop',
  'author': {
    'kind': kind,
    if (kind == 'human') 'name': 'You',
    if (kind == 'agent') 'agent': 'bot_iris',
  },
  'audience': {'kind': 'everyone'},
  'intent': intent,
  'body': body,
  'reply_to': ?replyTo,
  'sent_at': 0,
};

void main() {
  testWidgets(
    'desktop transcript starts at the top and keeps reply affordance',
    (tester) async {
      tester.view.physicalSize = const Size(1120, 760);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);

      final client = _Client();
      final store = _store(
        client,
        messages: [
          _message('m1', 'A short project update.'),
          _message('m2', 'The report is ready for review.', kind: 'agent'),
          _message('m3', 'I will check the final details.', replyTo: 'm2'),
        ],
      );
      final scroll = ScrollController();
      addTearDown(scroll.dispose);
      addTearDown(store.dispose);

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Transcript(
              store: store,
              controller: scroll,
              desktop: true,
              onReply: (_) {},
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      final first = tester.getTopLeft(find.byKey(const ValueKey('m1')));
      expect(first.dy, lessThan(120));
      expect(scroll.position.maxScrollExtent, 0);
      expect(
        find.text('iris: The report is ready for review.'),
        findsOneWidget,
      );
      expect(find.byTooltip('Reply to You'), findsNWidgets(2));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'desktop system notices expand without hiding the visible status',
    (tester) async {
      final client = _Client();
      const notice =
          'Room paused: turn budget exhausted. No further turns will run.';
      final store = _store(
        client,
        messages: [
          _message('notice', notice, kind: 'system', intent: 'system_notice'),
        ],
      );
      final scroll = ScrollController();
      addTearDown(scroll.dispose);
      addTearDown(store.dispose);

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Transcript(store: store, controller: scroll, desktop: true),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text(notice), findsOneWidget);

      await tester.tap(find.text(notice));
      await tester.pumpAndSettle();
      expect(find.byType(SelectableText), findsOneWidget);
      expect(find.text(notice), findsNWidgets(2));

      // Adding a message shifts this reverse sliver's child indices. The
      // expanded notice should retain its state through the keyed wrapper.
      client.emit({
        'type': 'event',
        'envelope': {
          'id': 'event_2',
          'seq': 2,
          'room': 'room_desktop',
          'at': 0,
          'event': {
            'event': 'message_posted',
            'message': _message('m2', 'Follow-up status update.'),
          },
        },
      });
      await tester.pumpAndSettle();
      expect(find.byType(SelectableText), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'desktop follows new messages and preserves a reader away from latest',
    (tester) async {
      tester.view.physicalSize = const Size(1120, 760);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);

      final client = _Client();
      final store = _store(
        client,
        messages: [
          for (var i = 1; i <= 24; i++)
            _message('m$i', 'Production handoff $i'),
        ],
      );
      final scroll = ScrollController();
      addTearDown(scroll.dispose);
      addTearDown(store.dispose);

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: ListenableBuilder(
              listenable: store,
              builder: (_, _) =>
                  Transcript(store: store, controller: scroll, desktop: true),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(scroll.position.maxScrollExtent, greaterThan(0));
      expect(find.text('Production handoff 24'), findsOneWidget);

      await tester.drag(find.byType(CustomScrollView), const Offset(0, 220));
      await tester.pumpAndSettle();
      final readingOffset = scroll.offset;
      expect(readingOffset, lessThan(scroll.position.maxScrollExtent));

      client.emit({
        'type': 'event',
        'envelope': {
          'id': 'event_25',
          'seq': 25,
          'room': 'room_desktop',
          'at': 0,
          'event': {
            'event': 'message_posted',
            'message': _message('m25', 'Production handoff 25'),
          },
        },
      });
      await tester.pumpAndSettle();
      expect(find.text('Jump to latest'), findsOneWidget);
      expect(scroll.offset, closeTo(readingOffset, 1));

      await tester.tap(find.text('Jump to latest'));
      await tester.pumpAndSettle();
      expect(find.text('Production handoff 25'), findsOneWidget);
      expect(scroll.offset, scroll.position.maxScrollExtent);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('desktop composer keeps reply context and sends accessibly', (
    tester,
  ) async {
    final controller = TextEditingController();
    addTearDown(controller.dispose);
    var sent = false;
    var cancelled = false;
    final original = ChatMessage(
      id: 'original',
      author: Author(kind: 'agent', agentId: 'bot_iris'),
      body: 'The release notes are ready.',
      intent: 'say',
      taskId: null,
      sentAt: Timestamp(0),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Composer(
            controller: controller,
            onSend: () => sent = true,
            onAttach: () {},
            desktop: true,
            replyTo: original,
            replyAuthor: 'Iris',
            onCancelReply: () => cancelled = true,
          ),
        ),
      ),
    );
    expect(find.text('Reply to Iris'), findsOneWidget);
    expect(find.text('The release notes are ready.'), findsOneWidget);
    expect(find.byTooltip('Add files or mention'), findsOneWidget);
    expect(find.byTooltip('Send message · Ctrl+Enter'), findsOneWidget);

    await tester.enterText(find.byType(TextField), 'Reviewed and approved.');
    await tester.pump();
    await tester.tap(find.byTooltip('Send message · Ctrl+Enter'));
    await tester.pump();
    expect(sent, isTrue);

    await tester.tap(find.byTooltip('Cancel reply'));
    expect(cancelled, isTrue);
    expect(tester.takeException(), isNull);
  });
}
