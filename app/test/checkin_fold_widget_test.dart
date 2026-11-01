import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/net/room_client.dart';
import 'package:orgbot_app/state/room_store.dart';
import 'package:orgbot_app/ui/room_widgets.dart';

class _Link implements RoomLink {
  final _m = StreamController<Map<String, Object?>>.broadcast(sync: true);
  @override
  Stream<Map<String, Object?>> get messages => _m.stream;
  @override
  Stream<LinkState> get connectionState => const Stream.empty();
  @override
  LinkState get current => LinkState.connected;
  @override
  void noSuchMethod(Invocation invocation) {}
}

Map<String, Object?> posted(int seq, String id, String agent, String body) => {
  'type': 'event',
  'envelope': {
    'seq': seq,
    'event': {
      'event': 'message_posted',
      'message': {
        'id': id,
        'author': {'kind': 'agent', 'agent': agent},
        'body': body,
        'intent': 'say',
        'sent_at': 1000 + seq,
      },
    },
  },
};

void main() {
  testWidgets('a run of check-ins shows as one line in the chat', (tester) async {
    final store = RoomStore(_Link());
    store.restore([
      {'type': 'snapshot', 'head': 0, 'roster': [
        {'id': 'bot_f', 'handle': 'grok-quick', 'status': 'idle'},
        {'id': 'bot_a', 'handle': 'astra', 'status': 'idle'},
      ], 'tasks': []},
      posted(1, 'm1', 'bot_f', 'I hold no task, so I am not starting work.'),
      posted(2, 'm2', 'bot_a', 'Aece, no new review is ready.'),
    ]);
    await tester.pumpWidget(MaterialApp(home: Scaffold(body: Transcript(store: store, controller: ScrollController(), desktop: true))));
    await tester.pumpAndSettle();
    expect(find.textContaining('2 bots checked in'), findsOneWidget);
    expect(find.text('Aece, no new review is ready.'), findsNothing);
    await tester.tap(find.textContaining('2 bots checked in'));
    await tester.pumpAndSettle();
    expect(find.text('Aece, no new review is ready.'), findsOneWidget);
    store.dispose();
  });
}
