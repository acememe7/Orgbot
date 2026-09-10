// Telling you when the room needs you.
//
// A room runs for days and almost nothing it does is worth interrupting
// someone for. The engine already decides which events are: a decision only
// you can make, a bot asking you something, the room finishing, the room
// stuck, the room out of turns. This delivers those and nothing else.
//
// The restraint is the point. A notification for every task transition trains
// you to swipe them away, and then the one that mattered goes with the rest.
// One room here sat stopped for six hours because nothing said so.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';

/// Why the room is asking for you, mirroring the engine's own list.
enum Attention {
  /// A decision only you can make.
  decision('A decision needs you'),

  /// Something material happened to the room's security posture.
  security('A security issue needs your judgement'),

  /// A bot asked you something directly.
  mentioned('A bot is asking you something'),

  /// The room used the turns it was given and stopped.
  budget('The room is out of turns'),

  /// The room stopped and cannot continue on its own.
  stuck('The room cannot continue'),

  /// An objective is done.
  milestone('An objective is done'),

  /// The room finished.
  finished('The room finished');

  const Attention(this.headline);

  /// The title shown on the notification.
  final String headline;

  /// Whether this should reach you now rather than wait to be found.
  ///
  /// Good news can wait; being asked for something, or being stopped, cannot.
  bool get isUrgent => this != milestone && this != finished;

  /// Parse the engine's own name for this.
  static Attention? parse(String? reason) => switch (reason) {
        'decision_needed' => decision,
        'security_incident' => security,
        'mentioned' => mentioned,
        'budget_spent' => budget,
        'room_stuck' => stuck,
        'milestone_reached' => milestone,
        'room_finished' => finished,
        _ => null,
      };
}

/// Delivers the handful of things worth interrupting someone for.
class Attentions {
  Attentions({FlutterLocalNotificationsPlugin? plugin})
      : _plugin = plugin ?? FlutterLocalNotificationsPlugin();

  final FlutterLocalNotificationsPlugin _plugin;
  bool _ready = false;

  /// Whether notifications are switched on. Off means nothing is delivered,
  /// and the app is otherwise unaffected.
  bool enabled = true;

  /// Notifications already shown, so a redelivered event does not buzz twice.
  ///
  /// A reconnecting client replays the log from wherever it left off, and
  /// without this every reconnection would repeat every alert in it.
  final Set<String> _shown = {};

  /// The most recent thing the room wanted, for showing in-app.
  ({Attention reason, String title, String body})? latest;

  /// Ask for permission and prepare the channel. Safe to call more than once.
  Future<void> start() async {
    if (_ready || !_supported) return;
    try {
      await _plugin.initialize(
        settings: const InitializationSettings(
          android: AndroidInitializationSettings('@mipmap/ic_launcher'),
        ),
      );
      final android = _plugin.resolvePlatformSpecificImplementation<
          AndroidFlutterLocalNotificationsPlugin>();
      await android?.requestNotificationsPermission();
      _ready = true;
    } catch (error) {
      // A device that refuses notifications must not stop the app working;
      // the room is still perfectly usable by looking at it.
      debugPrint('notifications unavailable: $error');
    }
  }

  /// Only Android delivers these today; elsewhere this is a no-op rather than
  /// a crash, so the same code runs on the desktop build.
  bool get _supported => !kIsWeb && Platform.isAndroid;

  /// Deliver one notification, if it is worth delivering and not a repeat.
  ///
  /// [key] identifies the underlying event so a replayed log is not shown
  /// twice.
  Future<void> show({
    required String key,
    required Attention reason,
    required String body,
  }) async {
    if (!enabled || !_shown.add(key)) return;
    latest = (reason: reason, title: reason.headline, body: body);
    if (!_ready || !_supported) return;

    try {
      await _plugin.show(
        id: key.hashCode & 0x7fffffff,
        title: reason.headline,
        body: body,
        notificationDetails: NotificationDetails(
          android: AndroidNotificationDetails(
            'orgbot_attention',
            'Things that need you',
            channelDescription:
                'Decisions, questions addressed to you, and a room that has '
                'stopped. Not ordinary progress.',
            importance:
                reason.isUrgent ? Importance.high : Importance.defaultImportance,
            priority: reason.isUrgent ? Priority.high : Priority.defaultPriority,
          ),
        ),
      );
    } catch (error) {
      debugPrint('could not show a notification: $error');
    }
  }

  /// Take a notification the engine produced, if this event carries one.
  ///
  /// The decision about what is worth telling you lives in the engine, beside
  /// the events themselves; this only delivers it. Two implementations of that
  /// judgement would drift, and the one that drifted would be the one that
  /// woke you at three in the morning.
  Future<void> consider(Map<String, Object?> message) async {
    final notification =
        (message['notification'] as Map?)?.cast<String, Object?>();
    if (notification == null) return;
    final reason = Attention.parse(notification['reason'] as String?);
    if (reason == null) return;
    final envelope = (message['envelope'] as Map?)?.cast<String, Object?>();
    await show(
      key: envelope?['id'] as String? ?? '${envelope?['seq'] ?? notification}',
      reason: reason,
      body: notification['body'] as String? ?? '',
    );
  }
}
