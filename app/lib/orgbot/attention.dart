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

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';

const _workspaceChannel = MethodChannel('orgbot/workspace');
const _activationChannel = MethodChannel('orgbot/activation');

/// Why the room is asking for you, mirroring the engine's own list.
enum Attention {
  /// A decision only you can make.
  decision('A decision needs you'),

  /// Something material happened to the room's security posture.
  security('A security issue needs your judgement'),

  /// A bot asked you something directly.
  mentioned('A bot is asking you something'),

  /// The room is still running, but some of its work has nowhere to go.
  ///
  /// Easy to miss without being told: from the outside the room looks busy.
  needsAttention('Some work has nowhere to go'),

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
    'needs_attention' => needsAttention,
    'budget_spent' => budget,
    'room_stuck' => stuck,
    'milestone_reached' => milestone,
    'room_finished' => finished,
    _ => null,
  };
}

/// Identifies the saved room a notification should bring forward.
class AttentionTarget {
  const AttentionTarget({
    required this.roomId,
    required this.reason,
    required this.eventId,
  });

  final String roomId;
  final Attention reason;
  final String eventId;

  Map<String, Object?> toJson() => {
    'room_id': roomId,
    'reason': reason.name,
    'event_id': eventId,
  };

  static AttentionTarget? fromPayload(String? payload) {
    if (payload == null || payload.isEmpty) return null;
    try {
      final decoded = jsonDecode(payload);
      if (decoded is! Map) return null;
      final roomId = decoded['room_id'];
      final reasonName = decoded['reason'];
      final eventId = decoded['event_id'];
      if (roomId is! String ||
          roomId.isEmpty ||
          reasonName is! String ||
          eventId is! String) {
        return null;
      }
      final reasons = Attention.values.where(
        (value) => value.name == reasonName,
      );
      if (reasons.isEmpty) return null;
      return AttentionTarget(
        roomId: roomId,
        reason: reasons.first,
        eventId: eventId,
      );
    } on FormatException {
      return null;
    }
  }
}

/// Delivers the handful of things worth interrupting someone for.
class Attentions {
  Attentions({FlutterLocalNotificationsPlugin? plugin})
    : _plugin = plugin ?? FlutterLocalNotificationsPlugin();

  final FlutterLocalNotificationsPlugin _plugin;

  void listenForDesktopActivation(List<String> arguments) {
    if (kIsWeb || !Platform.isLinux) return;
    for (var index = 0; index + 1 < arguments.length; index++) {
      if (arguments[index] == '--orgbot-attention') {
        openPayload(arguments[index + 1]);
        break;
      }
    }
    _activationChannel.setMethodCallHandler((call) async {
      if (call.method == 'open_attention' && call.arguments is String) {
        openPayload(call.arguments as String);
      }
    });
    unawaited(_announceActivationReady());
  }

  Future<void> _announceActivationReady() async {
    try {
      await _activationChannel.invokeMethod<void>('activation_ready');
    } on MissingPluginException {
      // Non-Linux test hosts and older runners have no native activation peer.
    } on PlatformException catch (error) {
      debugPrint('desktop activation unavailable: ${error.message}');
    }
  }

  void openPayload(String payload) {
    final target = AttentionTarget.fromPayload(payload);
    if (target == null) return;
    final callback = _onRouteRequested;
    if (callback == null) {
      _pendingRoute = target;
    } else {
      callback(target);
    }
  }

  bool _ready = false;
  Future<void>? _starting;
  void Function(AttentionTarget target)? _onRouteRequested;
  AttentionTarget? _pendingRoute;

  void Function(AttentionTarget target)? get onRouteRequested =>
      _onRouteRequested;

  /// Routes notification taps. A tap received before the app shell is ready is
  /// retained for it to consume once navigation is attached.
  set onRouteRequested(void Function(AttentionTarget target)? callback) {
    _onRouteRequested = callback;
    final pending = _pendingRoute;
    if (callback != null && pending != null) {
      _pendingRoute = null;
      callback(pending);
    }
  }

  AttentionTarget? takePendingRoute() {
    final pending = _pendingRoute;
    _pendingRoute = null;
    return pending;
  }

  /// Whether notifications are switched on. Off means nothing is delivered,
  /// and the app is otherwise unaffected.
  bool enabled = true;

  /// Notifications already shown, so a redelivered event does not buzz twice.
  ///
  /// A reconnecting client replays the log from wherever it left off, and
  /// without this every reconnection would repeat every alert in it.
  final Set<String> _shown = <String>{};
  static const _maxRememberedEvents = 512;

  /// The most recent thing the room wanted, for showing in-app.
  ({Attention reason, String title, String body})? latest;

  /// Ask for permission and prepare the channel. Safe to call more than once.
  Future<void> start() {
    if (_ready || !_supported) return Future<void>.value();
    return _starting ??= _initialize().whenComplete(() => _starting = null);
  }

  Future<void> _initialize() async {
    try {
      final initialized = await _plugin.initialize(
        settings: InitializationSettings(
          android: AndroidInitializationSettings('@mipmap/ic_launcher'),
          linux: LinuxInitializationSettings(
            defaultActionName: 'open',
            defaultIcon: AssetsLinuxIcon('assets/orgbots-desktop.png'),
          ),
        ),
        onDidReceiveNotificationResponse: handleNotificationResponse,
      );
      if (initialized == false) return;
      final android = _plugin
          .resolvePlatformSpecificImplementation<
            AndroidFlutterLocalNotificationsPlugin
          >();
      if (!kIsWeb && Platform.isAndroid) {
        await android?.requestNotificationsPermission();
      }
      _ready = true;
    } catch (error) {
      // A device that refuses notifications must not stop the app working;
      // the room is still perfectly usable by looking at it.
      debugPrint('notifications unavailable: $error');
    }
  }

  /// Linux uses the desktop notification service; Android keeps its existing
  /// notification channel.
  bool get _supported => !kIsWeb && (Platform.isAndroid || Platform.isLinux);

  /// Deliver one notification, if it is worth delivering and not a repeat.
  ///
  /// [key] identifies the underlying event so a replayed log is not shown
  /// twice.
  Future<void> show({
    required String key,
    required Attention reason,
    required String body,
    String? roomId,
  }) async {
    if (!enabled || !_remember(key)) return;
    latest = (reason: reason, title: reason.headline, body: body);
    if (!_supported) return;
    if (!_ready) await start();
    if (!enabled || !_ready) return;

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
            importance: reason.isUrgent
                ? Importance.high
                : Importance.defaultImportance,
            priority: reason.isUrgent
                ? Priority.high
                : Priority.defaultPriority,
          ),
          linux: LinuxNotificationDetails(
            category: LinuxNotificationCategory.im,
            urgency: switch (reason) {
              Attention.security ||
              Attention.stuck => LinuxNotificationUrgency.critical,
              Attention.decision ||
              Attention.mentioned ||
              Attention.needsAttention ||
              Attention.budget => LinuxNotificationUrgency.normal,
              Attention.milestone ||
              Attention.finished => LinuxNotificationUrgency.low,
            },
          ),
        ),
        payload: roomId == null
            ? null
            : jsonEncode(
                AttentionTarget(
                  roomId: roomId,
                  reason: reason,
                  eventId: key,
                ).toJson(),
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
  Future<void> consider(Map<String, Object?> message, {String? roomId}) async {
    final notification = (message['notification'] as Map?)
        ?.cast<String, Object?>();
    if (notification == null) return;
    final reason = Attention.parse(notification['reason'] as String?);
    if (reason == null) return;
    final envelope = (message['envelope'] as Map?)?.cast<String, Object?>();
    await show(
      key: envelope?['id'] as String? ?? '${envelope?['seq'] ?? notification}',
      reason: reason,
      body: notification['body'] as String? ?? '',
      roomId: roomId,
    );
  }

  bool _remember(String key) {
    if (!_shown.add(key)) return false;
    while (_shown.length > _maxRememberedEvents) {
      _shown.remove(_shown.first);
    }
    return true;
  }

  /// Dispatch a native notification click. Public for focused route tests.
  void handleNotificationResponse(NotificationResponse response) {
    if (response.notificationResponseType !=
        NotificationResponseType.selectedNotification) {
      return;
    }
    final target = AttentionTarget.fromPayload(response.payload);
    if (target == null) return;
    if (!kIsWeb && Platform.isLinux) {
      unawaited(_presentWindow());
    }
    final callback = _onRouteRequested;
    if (callback == null) {
      _pendingRoute = target;
    } else {
      callback(target);
    }
  }

  Future<void> _presentWindow() async {
    try {
      await _workspaceChannel.invokeMethod<void>('present_window');
    } on PlatformException {
      // Notification routing remains useful if the window has exited.
    } on MissingPluginException {
      // The native presentation hook is Linux-only.
    }
  }
}
