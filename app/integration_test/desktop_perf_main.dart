import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:orgbot_app/main.dart' as app;
import 'package:orgbot_app/orgbot/attention.dart';
import 'package:orgbot_app/net/room_client.dart';
import 'package:orgbot_app/orgbot/library.dart';
import 'package:orgbot_app/orgbot/models.dart';
import 'package:orgbot_app/orgbot/credentials.dart';
import 'package:orgbot_app/state/engine_room_controller.dart';
import 'package:orgbot_app/state/room_store.dart';
import 'package:orgbot_app/ui/orgbot_screen.dart';

class _FakeRoomLink implements RoomLink {
  final _messages = StreamController<Map<String, Object?>>.broadcast(
    sync: true,
  );
  final _states = StreamController<LinkState>.broadcast(sync: true);

  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;
  @override
  Stream<LinkState> get connectionState => _states.stream;
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
  void stopNow({String reason = ''}) {}
  @override
  void resume() {}
  @override
  void resync() {}
  @override
  Future<void> connect(String url, {String? token}) async {}
  @override
  Future<void> close() async {
    await _messages.close();
    await _states.close();
  }

  void emit(Map<String, Object?> message) => _messages.add(message);
}

class _PreviewRoom extends EngineRoomController {
  _PreviewRoom({
    required super.room,
    required super.library,
    required this.sample,
  }) {
    phase = RoomLaunch.running;
    sample.addListener(_relaySample);
  }

  final RoomStore sample;

  void _relaySample() => notifyListeners();

  @override
  RoomStore get store => sample;

  @override
  Future<void> start() async {}

  @override
  void dispose() {
    sample.removeListener(_relaySample);
    super.dispose();
  }
}

Map<String, Object?> _stream(String agent, Map<String, Object?> event) => {
  'type': 'stream',
  'agent': agent,
  'event': event,
};


const _bodies = [
  'Thanks, Aece. I checked the **release notes** and the build script. Two things stand out:\n\n1. The `make-deb.sh` step copies the engine before it is stripped.\n2. The desktop entry still points at the old icon path.\n\nI can fix both in one change if you want.',
  'Short update: tests pass locally.',
  'Here is the patch I propose:\n\n```rust\nfn room_server_name() -> &\'static str {\n    "orgbot_room"\n}\n```\n\n@opus can you double-check the permission rule spelling?',
  'Reviewed. No blocking findings. Minor: the error message in `supervisor.rs` could name the unit so logs are searchable.',
  '> The board is empty.\n\nAgreed. Waiting for a real goal before splitting work.',
  'I looked at the [Flutter docs](https://docs.flutter.dev) on scroll anchoring; the current approach matches their guidance.',
];

Map<String, Object?> _rich(int seq, String author) => {
  'type': 'event',
  'envelope': {
    'id': 'fixture-$seq',
    'seq': seq,
    'room': 'activity-preview',
    'at': DateTime.now().millisecondsSinceEpoch,
    'event': {
      'event': 'message_posted',
      'message': {
        'id': 'message-$seq',
        'room': 'activity-preview',
        'author': seq % 7 == 0
            ? {'kind': 'human', 'name': 'Aece'}
            : {'kind': 'agent', 'agent': author},
        'audience': {'kind': 'everyone'},
        'intent': 'say',
        'body': _bodies[seq % _bodies.length],
        'sent_at': DateTime.now().millisecondsSinceEpoch,
        'files': <Object?>[],
      },
    },
  },
};

/// Finds the first state of type [T] in the live tree that passes [test].
T? _find<T extends State>(bool Function(T state) test) {
  T? found;
  void visit(Element element) {
    if (found != null) return;
    if (element is StatefulElement && element.state is T && test(element.state as T)) {
      found = element.state as T;
      return;
    }
    element.visitChildren(visit);
  }
  WidgetsBinding.instance.rootElement?.visitChildren(visit);
  return found;
}

Future<void> _frames(int count) async {
  for (var i = 0; i < count; i++) {
    await WidgetsBinding.instance.endOfFrame;
  }
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final phases = <String, List<ui.FrameTiming>>{};
  final clock = <String, Stopwatch>{};
  var phaseName = 'warmup';
  void enter(String next) {
    clock[phaseName]?.stop();
    phaseName = next;
    clock[next] = Stopwatch()..start();
  }
  void onTimings(List<ui.FrameTiming> frames) =>
      (phases[phaseName] ??= []).addAll(frames);

  final link = _FakeRoomLink();
  final store = RoomStore(link, notificationRoomId: 'activity-preview');
  const names = ['Astra', 'Julian', 'Miles', 'Finn', 'Devon', 'Rhea', 'Quinn', 'Sol', 'Mira', 'Ellis'];
  final bots = List.generate(10, (i) => 'bot_$i');
  link.emit({
    'type': 'snapshot',
    'room': {'id': 'activity-preview', 'spec': {'name': 'Perf room', 'goal': 'Measure'}, 'phase': 'running'},
    'head': 0,
    'roster': [
      for (var i = 0; i < bots.length; i++)
        {'id': bots[i], 'handle': 'agent$i', 'display_name': names[i],
         'roles': i == 0 ? ['coordinator'] : ['engineer', 'backend'], 'status': 'idle'},
    ],
    'tasks': [],
  });
  for (var i = 1; i <= 500; i++) {
    link.emit(_rich(i, bots[i % bots.length]));
  }

  final library = Library(saver: MemorySaver());
  await library.load();
  final credentials = Credentials();
  final workspace = Directory.systemTemp.createTempSync('orgbots-perf-$pid-');
  final slots = <RoomSlot>[];
  for (var i = 0; i < bots.length; i++) {
    final bot = library.addBot(BotDef(name: names[i], handle: 'agent$i',
        provider: ProviderKind.scripted, role: i == 0 ? 'coordinator' : 'engineer'));
    slots.add(RoomSlot(bot.id));
  }
  final room = library.addRoom(RoomDef(id: 'activity-preview', name: 'Perf room',
      goal: 'Measure', path: workspace.path, maxTurns: 0, bots: slots));
  library.sessions[room.id] = _PreviewRoom(room: room, library: library, sample: store);

  final themeName = Platform.environment['PERF_THEME'] ?? 'original';
  runApp(MaterialApp(
    debugShowCheckedModeBanner: false,
    theme: app.orgBotTheme(Brightness.dark, desktop: true),
    darkTheme: app.orgBotTheme(Brightness.dark, desktop: true),
    themeMode: ThemeMode.dark,
    home: OrgBotScreen(library: library, credentials: credentials, desktopClis: true),
  ));
  WidgetsBinding.instance.addPostFrameCallback((_) {
    RoomStore.attentions.onRouteRequested?.call(AttentionTarget(
        roomId: room.id, reason: Attention.decision, eventId: 'perf-open'));
  });
  await Future<void>.delayed(const Duration(seconds: 3));
  WidgetsBinding.instance.addTimingsCallback(onTimings);

  // Idle: nothing should be drawing.
  enter('idle');
  await Future<void>.delayed(const Duration(seconds: 3));

  // Streaming: four bots thinking, tools starting and ending, messages landing.
  enter('streaming');
  var sequence = 0;
  var seq = 500;
  final burst = Timer.periodic(const Duration(milliseconds: 120), (_) {
    sequence++;
    final agent = bots[sequence % 4];
    link.emit(_stream(agent, {'type': 'thinking'}));
    link.emit(_stream(agent, {'type': 'text_delta', 'text': 'token $sequence'}));
    if (sequence % 3 == 0) {
      final id = 'tool-$sequence';
      link.emit(_stream(agent, {'type': 'tool_use_start', 'id': id, 'name': 'mcp__orgbot_room__list_tasks'}));
      Timer(const Duration(milliseconds: 60), () =>
          link.emit(_stream(agent, {'type': 'tool_use_end', 'id': id, 'succeeded': true})));
    }
    if (sequence % 8 == 0) link.emit(_rich(++seq, agent));
  });
  await Future<void>.delayed(const Duration(seconds: 6));
  burst.cancel();
  for (final agent in bots) {
    link.emit(_stream(agent, {'type': 'done'}));
  }
  await Future<void>.delayed(const Duration(milliseconds: 500));

  // Scrolling the transcript, the way a wheel or trackpad does: small steps
  // every frame, up through history and back.
  final transcript = _find<ScrollableState>((s) =>
      s.widget.axisDirection == AxisDirection.down &&
      s.position.hasContentDimensions &&
      s.position.maxScrollExtent > 2000);
  enter('scroll');
  if (transcript != null) {
    final position = transcript.position;
    for (var i = 0; i < 240; i++) {
      position.jumpTo((position.pixels - 60).clamp(position.minScrollExtent, position.maxScrollExtent));
      await WidgetsBinding.instance.endOfFrame;
    }
    for (var i = 0; i < 240; i++) {
      position.jumpTo((position.pixels + 60).clamp(position.minScrollExtent, position.maxScrollExtent));
      await WidgetsBinding.instance.endOfFrame;
    }
  }

  // Hovering down the message list.
  enter('hover');
  final size = WidgetsBinding.instance.platformDispatcher.views.first.physicalSize /
      WidgetsBinding.instance.platformDispatcher.views.first.devicePixelRatio;
  for (var i = 0; i < 180; i++) {
    final y = 120 + (i * 7) % (size.height - 260);
    GestureBinding.instance.handlePointerEvent(PointerHoverEvent(
        position: Offset(size.width * 0.45, y), kind: PointerDeviceKind.mouse));
    await WidgetsBinding.instance.endOfFrame;
  }

  // Typing into the composer, including an @mention list.
  enter('typing');
  final field = _find<EditableTextState>((_) => true);
  if (field != null) {
    const text = '@ag Can everyone confirm the build passes on their side before I tag the release?';
    for (var i = 1; i <= text.length; i++) {
      field.widget.controller.value = TextEditingValue(
          text: text.substring(0, i), selection: TextSelection.collapsed(offset: i));
      await _frames(2);
    }
    field.widget.controller.clear();
  }

  // Switching side panels from the tool rail.
  enter('panels');
  for (var round = 0; round < 6; round++) {
    for (var tool = 0; tool < 4; tool++) {
      final button = _find<State>((s) => s.widget.key == ValueKey('workspace-tool-$tool'));
      final element = button?.context as Element?;
      IconButton? icon;
      element?.visitChildren((child) {});
      void look(Element e) {
        if (icon != null) return;
        if (e.widget is IconButton) { icon = e.widget as IconButton; return; }
        e.visitChildren(look);
      }
      if (element != null) look(element);
      icon?.onPressed?.call();
      await _frames(20);
    }
  }
  enter('done');
  await _frames(3);
  WidgetsBinding.instance.removeTimingsCallback(onTimings);

  Map<String, Object> summary(String name, List<ui.FrameTiming> frames) {
    final seconds = (clock[name]?.elapsedMicroseconds ?? 0) / 1e6;
    int pct(List<int> v, double f) => v.isEmpty ? 0 : v[(v.length * f).floor().clamp(0, v.length - 1)];
    final build = frames.map((f) => f.buildDuration.inMicroseconds).toList()..sort();
    final raster = frames.map((f) => f.rasterDuration.inMicroseconds).toList()..sort();
    final total = frames.map((f) => f.totalSpan.inMicroseconds).toList()..sort();
    return {
      'frames': frames.length,
      'build_p50': pct(build, .5), 'build_p90': pct(build, .9), 'build_p99': pct(build, .99),
      'build_max': build.isEmpty ? 0 : build.last,
      'raster_p50': pct(raster, .5), 'raster_p90': pct(raster, .9), 'raster_p99': pct(raster, .99),
      'raster_max': raster.isEmpty ? 0 : raster.last,
      'over_16ms': total.where((t) => t > 16667).length,
      'fps': seconds == 0 ? 0 : (frames.length / seconds).round(),
    };
  }
  // ignore: avoid_print
  print('PERF_RESULT ${jsonEncode({
    'theme': themeName,
    'transcript_found': transcript != null,
    'composer_found': field != null,
    'env': {for (final k in ['GDK_GL', '__GL_SYNC_TO_VBLANK', 'GDK_BACKEND']) k: Platform.environment[k] ?? ''},
    for (final entry in phases.entries) entry.key: summary(entry.key, entry.value),
  })}');
  exit(0);
}
