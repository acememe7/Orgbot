// What is worth interrupting someone for.
//
// The judgement lives in the engine; this asserts the delivery side keeps its
// side of the bargain — nothing without a reason, nothing twice, and nothing
// at all when it is switched off.

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:orgbot_app/orgbot/attention.dart';

Map<String, Object?> event({
  String id = 'ev_1',
  String? reason,
  String body = 'something happened',
}) => {
  'type': 'event',
  'envelope': {
    'id': id,
    'seq': 1,
    'event': {'event': 'message_posted'},
  },
  if (reason != null) 'notification': {'reason': reason, 'body': body},
};

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  group('what gets through', () {
    test('an event with no notification tells nobody', () async {
      final attentions = Attentions();
      await attentions.consider(event());
      expect(attentions.latest, isNull);
    });

    test(
      'a reason the app does not know is ignored rather than guessed at',
      () async {
        final attentions = Attentions();
        await attentions.consider(event(reason: 'something_new'));
        expect(attentions.latest, isNull);
      },
    );

    test('a decision reaches you', () async {
      final attentions = Attentions();
      await attentions.consider(
        event(reason: 'decision_needed', body: 'it provisions a paid VPS'),
      );
      expect(attentions.latest?.reason, Attention.decision);
      expect(attentions.latest?.body, 'it provisions a paid VPS');
      expect(attentions.latest?.title, 'A decision needs you');
    });

    /// The stall that started this: a room out of turns said nothing and sat
    /// for six hours.
    test('a room out of turns says so', () async {
      final attentions = Attentions();
      await attentions.consider(event(reason: 'budget_spent'));
      expect(attentions.latest?.reason, Attention.budget);
      expect(attentions.latest?.reason.isUrgent, isTrue);
    });

    test('a bot asking something is urgent; a milestone can wait', () {
      expect(Attention.mentioned.isUrgent, isTrue);
      expect(Attention.stuck.isUrgent, isTrue);
      expect(Attention.milestone.isUrgent, isFalse);
      expect(Attention.finished.isUrgent, isFalse);
    });
  });

  group('not twice, and not when off', () {
    /// A reconnecting client replays the log; without de-duplication every
    /// reconnection would repeat every alert in it.
    test('the same event is delivered once', () async {
      final attentions = Attentions();
      await attentions.consider(event(reason: 'decision_needed'));
      attentions.latest = null;
      await attentions.consider(event(reason: 'decision_needed'));
      expect(attentions.latest, isNull, reason: 'already seen');
    });

    test('a different event does get through', () async {
      final attentions = Attentions();
      await attentions.consider(event(id: 'ev_1', reason: 'decision_needed'));
      attentions.latest = null;
      await attentions.consider(event(id: 'ev_2', reason: 'room_stuck'));
      expect(attentions.latest?.reason, Attention.stuck);
    });

    test('switching them off delivers nothing', () async {
      final attentions = Attentions()..enabled = false;
      await attentions.consider(event(reason: 'decision_needed'));
      expect(attentions.latest, isNull);
    });
  });

  group('notification routing', () {
    test('a click waits for the app route handler and is consumed once', () {
      final attentions = Attentions();
      const target = AttentionTarget(
        roomId: 'room_saved_1',
        reason: Attention.decision,
        eventId: 'event_1',
      );
      attentions.handleNotificationResponse(
        NotificationResponse(
          notificationResponseType:
              NotificationResponseType.selectedNotification,
          payload: jsonEncode(target.toJson()),
        ),
      );

      final queued = attentions.takePendingRoute();
      expect(queued?.roomId, target.roomId);
      expect(queued?.reason, target.reason);
      expect(queued?.eventId, target.eventId);
      expect(attentions.takePendingRoute(), isNull);
    });

    test('clicks dispatch to the registered route handler', () {
      final attentions = Attentions();
      const target = AttentionTarget(
        roomId: 'room_saved_2',
        reason: Attention.stuck,
        eventId: 'event_2',
      );
      final routed = <AttentionTarget>[];
      attentions.onRouteRequested = routed.add;

      attentions.handleNotificationResponse(
        NotificationResponse(
          notificationResponseType:
              NotificationResponseType.selectedNotification,
          payload: jsonEncode(target.toJson()),
        ),
      );

      expect(routed, hasLength(1));
      expect(routed.single.roomId, target.roomId);
      expect(routed.single.reason, Attention.stuck);
    });

    test('malformed payloads and action responses do not route', () {
      final attentions = Attentions();
      final routed = <AttentionTarget>[];
      attentions.onRouteRequested = routed.add;
      attentions.handleNotificationResponse(
        const NotificationResponse(
          notificationResponseType:
              NotificationResponseType.selectedNotification,
          payload: '{invalid',
        ),
      );
      attentions.handleNotificationResponse(
        const NotificationResponse(
          notificationResponseType:
              NotificationResponseType.selectedNotificationAction,
          payload:
              '{"room_id":"room_saved_3","reason":"decision",'
              '"event_id":"event_3"}',
          actionId: 'mark-read',
        ),
      );

      expect(routed, isEmpty);
    });
  });

  /// Every reason the engine can send must have a mapping, or an alert would
  /// be silently dropped.
  test('every engine reason is understood', () {
    for (final reason in [
      'decision_needed',
      'security_incident',
      'mentioned',
      'budget_spent',
      'room_stuck',
      'milestone_reached',
      'room_finished',
    ]) {
      expect(Attention.parse(reason), isNotNull, reason: reason);
    }
  });
}
