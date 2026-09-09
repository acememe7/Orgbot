// The objectives view, and the wire it arrives on.
//
// The digest is computed by the engine and sent whole. These assert the Dart
// half of that contract: that the JSON the Rust side emits parses into the
// model the UI reads, and that the view leads with outcomes rather than
// records.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/net/room_client.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/state/room_store.dart';
import 'package:orgbot_app/ui/widgets/objectives_pane.dart';

/// A client that never connects; the tests push frames in by hand.
class _Silent implements RoomClient {
  final _messages = StreamController<Map<String, Object?>>.broadcast();
  final _state = StreamController<LinkState>.broadcast();

  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;
  @override
  Stream<LinkState> get connectionState => _state.stream;
  @override
  LinkState get current => LinkState.connected;
  @override
  void send(Map<String, Object?> message) {}
  @override
  void sendMessage(String author, String body, {String? taskId}) {}
  @override
  void answerBot(String taskId, String answer) {}
  @override
  void answerDecision(String decision, String chosen, {String note = ''}) =>
      answered.add((decision: decision, chosen: chosen));
  final answered = <({String decision, String chosen})>[];
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

/// A store fed directly, with no transport behind it.
RoomStore storeWith([List<Map<String, Object?>> frames = const []]) {
  final store = RoomStore(_Silent());
  store.restore(frames);
  return store;
}

/// The shape orgbot-engine's RoomDigest serialises to.
Map<String, Object?> digestJson({
  int objectives = 2,
  int needsYou = 0,
  int rawRecords = 18,
  bool expansion = false,
}) =>
    {
      'objectives': [
        for (var i = 0; i < objectives; i++)
          {
            'task': 'task_$i',
            'title': 'objective $i',
            'progress': {'done': 2, 'total': 4},
            'status': i == 0 && needsYou > 0 ? 'needs you' : 'in progress',
            'blocker': i == 0 ? 'waiting on B3-C source recovery' : null,
            'owners': ['bot_1'],
            'needs_operator': i == 0 && needsYou > 0,
            'records': ['task_r1', 'task_r2'],
          },
      ],
      'counts': {
        'objectives': objectives,
        'active': 1,
        'waiting_internally': 1,
        'needs_you': needsYou,
        'delivered': 1,
        'raw_records': rawRecords,
      },
      'needs_you': [
        for (var i = 0; i < needsYou; i++)
          {
            'id': 'dec_${i + 1}',
            'task': 'task_$i',
            'title': 'provision the VPS',
            'because': 'it provisions paid infrastructure',
            'options': ['Approve', 'Decline'],
          },
      ],
      'critical_path': ['task_0', 'task_1'],
      'health': {
        'inflation_ratio': expansion ? 9.0 : 1.5,
        'supporting_records': 12,
        'closed_as_duplicate': 1,
        'coordination_expansion': expansion,
      },
    };

void main() {
  group('parsing what the engine sends', () {
    test('a digest round-trips from the engine JSON', () {
      final digest = RoomDigest.fromJson(digestJson(needsYou: 1));

      expect(digest.counts.objectives, 2);
      expect(digest.counts.rawRecords, 18);
      expect(digest.objectives.first.title, 'objective 0');
      expect(digest.objectives.first.progress.label, '2 / 4 stages');
      expect(digest.objectives.first.progress.fraction, 0.5);
      expect(digest.objectives.first.needsOperator, isTrue);
      expect(digest.needsYou.single.because,
          'it provisions paid infrastructure');
      expect(digest.needsYou.single.options, ['Approve', 'Decline']);
      expect(digest.criticalPath, ['task_0', 'task_1']);
    });

    /// A room from before the digest existed must not crash the view.
    test('a snapshot with no digest leaves an empty one', () {
      final digest = RoomDigest.fromJson(const {});
      expect(digest.isEmpty, isTrue);
      expect(digest.counts.objectives, 0);
      expect(digest.health.coordinationExpansion, isFalse);
    });

    test('progress with nothing broken down says so', () {
      const nothing = Progress(done: 0, total: 0);
      expect(nothing.fraction, isNull);
      expect(nothing.label, 'not broken down yet');
    });
  });

  group('the store', () {
    test('a digest message replaces the view wholesale', () {
      final store = storeWith();
      expect(store.digest.isEmpty, isTrue);

      store.restore([
        {'type': 'digest', 'digest': digestJson(objectives: 3)}
      ]);
      expect(store.digest.counts.objectives, 3);

      // A later, smaller digest replaces rather than merges: the digest is a
      // fold over every task and cannot be patched.
      store.restore([
        {'type': 'digest', 'digest': digestJson(objectives: 1)}
      ]);
      expect(store.digest.objectives.length, 1);
    });

    test('a snapshot carries the first digest', () {
      final store = storeWith([
        {
          'type': 'snapshot',
          'room': {'id': 'room_1', 'name': 'test', 'goal': 'g'},
          'roster': [],
          'tasks': [],
          'digest': digestJson(),
        }
      ]);
      expect(store.digest.counts.objectives, 2);
    });
  });

  group('the view', () {
    Widget wrap(RoomStore store) => MaterialApp(
          home: Scaffold(body: ObjectivesPane(store: store)),
        );

    testWidgets('it leads with outcomes, not the record count',
        (tester) async {
      final store = storeWith([{'type': 'digest', 'digest': digestJson()}]);
      await tester.pumpWidget(wrap(store));

      expect(find.text('objective 0'), findsOneWidget);
      expect(find.text('objectives done'), findsOneWidget);
      // The raw total is available, but never the headline.
      expect(find.text('Show all 18 work records'), findsNothing,
          reason: 'no callback was given, so the affordance is hidden');
    });

    testWidgets('a quiet room says nothing needs you', (tester) async {
      final store = storeWith([{'type': 'digest', 'digest': digestJson()}]);
      await tester.pumpWidget(wrap(store));

      expect(find.text('Nothing needs you.'), findsOneWidget);
      expect(find.text('Needs you'), findsNothing, reason: 'no inbox needed');
    });

    testWidgets('a real decision is surfaced with its reason and options',
        (tester) async {
      final store = storeWith([
        {'type': 'digest', 'digest': digestJson(needsYou: 1)}
      ]);
      await tester.pumpWidget(wrap(store));

      expect(find.text('1 decision needs you.'), findsOneWidget);
      expect(find.text('Needs you'), findsOneWidget);
      expect(
        find.text(
            'Decision required because: it provisions paid infrastructure'),
        findsOneWidget,
      );
      expect(find.text('Approve'), findsOneWidget);
    });

    /// Every card says it, so nobody has to work it out.
    testWidgets('each objective states whether it needs you', (tester) async {
      final store = storeWith([
        {'type': 'digest', 'digest': digestJson(needsYou: 1)}
      ]);
      await tester.pumpWidget(wrap(store));

      expect(find.text('Needs you: YES'), findsOneWidget);
      expect(find.text('Needs you: no'), findsOneWidget);
    });

    testWidgets('the records behind an objective are one tap away',
        (tester) async {
      final store = storeWith([{'type': 'digest', 'digest': digestJson()}]);
      await tester.pumpWidget(wrap(store));

      expect(find.text('2 records behind it'), findsNWidgets(2));
      await tester.tap(find.text('2 records behind it').first);
      await tester.pumpAndSettle();
      expect(find.text('Hide the work behind it'), findsOneWidget);
    });

    testWidgets('runaway coordination is called out', (tester) async {
      final store = storeWith([
        {'type': 'digest', 'digest': digestJson(expansion: true)}
      ]);
      await tester.pumpWidget(wrap(store));

      // It sits below the objectives, so the list has to build that far.
      await tester.scrollUntilVisible(
        find.text('Coordination expansion'),
        250,
        scrollable: find.byType(Scrollable).first,
      );
      expect(find.text('Coordination expansion'), findsOneWidget);
      expect(find.textContaining('9.0 records per objective'), findsOneWidget);
    });

    testWidgets('the full board is reachable when a way there is given',
        (tester) async {
      final store = storeWith([{'type': 'digest', 'digest': digestJson()}]);
      var opened = false;
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: ObjectivesPane(
            store: store,
            onShowAllRecords: () => opened = true,
          ),
        ),
      ));

      await tester.scrollUntilVisible(
        find.text('Show all 18 work records'),
        250,
        scrollable: find.byType(Scrollable).first,
      );
      await tester.tap(find.text('Show all 18 work records'));
      expect(opened, isTrue, reason: 'compression is a layer, not deletion');
    });

    testWidgets('an empty room explains itself', (tester) async {
      final store = storeWith();
      await tester.pumpWidget(wrap(store));
      expect(find.text('Nothing on the board yet'), findsOneWidget);
    });
  });

  _answerTests();
  _viewTests();
  _graphTests();
}

// ------------------------------------------------------- answering a decision

void _answerTests() {
  group('answering', () {
    testWidgets('one tap answers the question, not the card', (tester) async {
      final client = _Silent();
      final store = RoomStore(client);
      store.restore([
        {'type': 'digest', 'digest': digestJson(needsYou: 1)}
      ]);
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(body: ObjectivesPane(store: store)),
      ));

      await tester.tap(find.text('Approve'));
      await tester.pump();

      expect(client.answered, hasLength(1));
      expect(client.answered.single.chosen, 'Approve');
      expect(
        client.answered.single.decision,
        'dec_1',
        reason: 'the answer is addressed to the question, not to a task',
      );
    });
  });
}

// ----------------------------------------------------------------- the views

void _viewTests() {
  group('views', () {
    Widget wrap(RoomStore store) => MaterialApp(
          home: Scaffold(body: ObjectivesPane(store: store)),
        );

    /// The point of the split: a real decision must not sit among ninety-five
    /// records that are all fine.
    testWidgets('Needs me shows only what waits on a person', (tester) async {
      final store = storeWith([
        {'type': 'digest', 'digest': digestJson(objectives: 4, needsYou: 1)}
      ]);
      await tester.pumpWidget(wrap(store));

      expect(find.text('objective 1'), findsOneWidget);
      await tester.tap(find.text('Needs me 1'));
      await tester.pumpAndSettle();

      expect(find.text('objective 0'), findsOneWidget, reason: 'the one that needs you');
      expect(find.text('objective 1'), findsNothing, reason: 'and nothing else');
    });

    /// An empty slice explains itself, and the good case says so warmly.
    testWidgets('an empty Needs me is reassuring, not blank', (tester) async {
      final store = storeWith([
        {'type': 'digest', 'digest': digestJson()}
      ]);
      await tester.pumpWidget(wrap(store));

      await tester.tap(find.text('Needs me 0'));
      await tester.pumpAndSettle();
      expect(
        find.textContaining('Nothing needs you'),
        findsWidgets,
      );
    });

    testWidgets('the chain deciding completion is shown', (tester) async {
      final store = storeWith([
        {'type': 'digest', 'digest': digestJson()}
      ]);
      await tester.pumpWidget(wrap(store));

      await tester.scrollUntilVisible(
        find.text('What decides when this finishes'),
        250,
        scrollable: find.byType(Scrollable).first,
      );
      expect(find.text('What decides when this finishes'), findsOneWidget);
    });

    testWidgets('who is doing what is shown, and idle capacity called out',
        (tester) async {
      final digest = digestJson();
      digest['utilization'] = {
        'bots': [
          {'agent': 'bot_1', 'handle': 'eng1', 'doing': 'working', 'holding': 2, 'free': false},
          {'agent': 'bot_2', 'handle': 'eng2', 'doing': 'idle', 'holding': 0, 'free': true},
        ],
        'idle': 1,
        'unclaimed': 4,
      };
      final store = storeWith([{'type': 'digest', 'digest': digest}]);
      await tester.pumpWidget(wrap(store));

      await tester.scrollUntilVisible(
        find.text('Who is doing what'),
        250,
        scrollable: find.byType(Scrollable).first,
      );
      expect(find.text('@eng1 · working'), findsOneWidget);
      expect(find.text('@eng2 · idle'), findsOneWidget);
      expect(
        find.textContaining('1 free while 4 piece(s) of work sit unclaimed'),
        findsOneWidget,
      );
    });

    /// Detail views stay focused: no critical path or roster strip there.
    testWidgets('the extra panels belong to the overview only', (tester) async {
      final store = storeWith([
        {'type': 'digest', 'digest': digestJson(objectives: 3)}
      ]);
      await tester.pumpWidget(wrap(store));

      await tester.tap(find.text('Active 1'));
      await tester.pumpAndSettle();
      expect(find.text('What decides when this finishes'), findsNothing);
      expect(find.text('Who is doing what'), findsNothing);
    });
  });
}

// ------------------------------------------------------- the expanded graph

void _graphTests() {
  group('the graph', () {
    Widget wrap(RoomStore store) => MaterialApp(
          home: Scaffold(body: ObjectivesPane(store: store)),
        );

    RoomStore withBoard() => storeWith([
          {
            'type': 'snapshot',
            'room': {'id': 'room_1', 'name': 'test', 'goal': 'g'},
            'roster': [],
            'tasks': [
              {'id': 'task_r1', 'title': 'rebuild', 'state': 'ready', 'depends_on': ['task_r2']},
              {'id': 'task_r2', 'title': 'recover the source', 'state': 'in_progress'},
            ],
            'digest': digestJson(objectives: 1),
          }
        ]);

    /// A chain should read as a chain, not as a list that happens to be in
    /// order.
    testWidgets('expanding a card shows what each record waits on',
        (tester) async {
      final store = withBoard();
      await tester.pumpWidget(wrap(store));

      await tester.tap(find.text('2 records behind it'));
      await tester.pumpAndSettle();

      expect(find.text('rebuild'), findsOneWidget);
      expect(find.text('↳ waits on recover the source'), findsOneWidget);
    });

    /// Work nobody has picked up looks exactly like work in progress unless
    /// something says otherwise.
    testWidgets('an unaccepted delegation is called out', (tester) async {
      final store = storeWith([
        {
          'type': 'snapshot',
          'room': {'id': 'room_1', 'name': 'test', 'goal': 'g'},
          'roster': [],
          'tasks': [
            {'id': 'task_r1', 'title': 'rebuild', 'state': 'blocked'},
            {'id': 'task_r2', 'title': 'recover', 'state': 'ready'},
          ],
          'digest': digestJson(objectives: 1),
        },
        {
          'type': 'event',
          'envelope': {
            'id': 'ev_1',
            'seq': 1,
            'room': 'room_1',
            'at': 0,
            'event': {
              'event': 'handoff_offered',
              'handoff': {
                'id': 'ho_1',
                'task': 'task_r1',
                'from': 'bot_1',
                'kind': 'assist',
                'state': 'offered',
                'brief': 'please help',
              },
            },
          },
        },
      ]);
      await tester.pumpWidget(wrap(store));

      await tester.tap(find.text('2 records behind it'));
      await tester.pumpAndSettle();

      expect(
        find.text('↳ handed off — nobody has picked this up yet'),
        findsOneWidget,
      );
    });

    test('a handoff says where it has got to', () {
      final offered = Handoff(
        id: 'ho_1',
        taskId: 'task_1',
        from: 'bot_1',
        kind: 'assist',
        state: 'offered',
        brief: '',
      );
      expect(offered.awaitingAcceptance, isTrue);
      expect(offered.progress, 'nobody has picked this up yet');

      final accepted = Handoff(
        id: 'ho_1',
        taskId: 'task_1',
        from: 'bot_1',
        kind: 'assist',
        state: 'accepted',
        brief: '',
        acceptedBy: 'bot_2',
      );
      expect(accepted.awaitingAcceptance, isFalse);
      expect(accepted.progress, 'accepted and being worked on');
    });
  });
}
