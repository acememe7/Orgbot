import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

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

Map<String, Object?> _event(int seq, String author, String body) => {
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
        'author': {'kind': 'agent', 'agent': author},
        'audience': {'kind': 'everyone'},
        'intent': 'say',
        'body': body,
        'sent_at': DateTime.now().millisecondsSinceEpoch,
        'files': <Object?>[],
      },
    },
  },
};

Map<String, Object?> _stream(String agent, Map<String, Object?> event) => {
  'type': 'stream',
  'agent': agent,
  'event': event,
};

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final timings = <({int build, int raster})>[];
  void onTimings(List<ui.FrameTiming> frames) {
    for (final frame in frames) {
      timings.add((
        build: frame.buildDuration.inMicroseconds,
        raster: frame.rasterDuration.inMicroseconds,
      ));
    }
  }

  final link = _FakeRoomLink();
  final store = RoomStore(link, notificationRoomId: 'activity-preview');
  const names = [
    'Astra',
    'Julian',
    'Miles',
    'Finn',
    'Devon',
    'Rhea',
    'Quinn',
    'Sol',
    'Mira',
    'Ellis',
  ];
  final bots = List.generate(10, (i) => 'bot_$i');
  link.emit({
    'type': 'snapshot',
    'room': {
      'id': 'activity-preview',
      'spec': {'name': 'Activity profile', 'goal': 'Preview tool activity'},
      'phase': 'running',
    },
    'head': 0,
    'roster': [
      for (var i = 0; i < bots.length; i++)
        {
          'id': bots[i],
          'handle': 'agent$i',
          'display_name': names[i],
          'roles': i == 0 ? ['coordinator'] : ['engineer'],
          'status': 'working',
        },
    ],
    'tasks': [],
    'digest': {
      'needs_you': [
        {
          'id': 'decision-preview',
          'task': 'task-preview',
          'title': 'Choose the next research direction',
          'because': 'Two promising directions have different tradeoffs.',
          'options': ['Study the workflow', 'Compare existing tools'],
        },
      ],
    },
  });
  for (var i = 1; i <= 500; i++) {
    link.emit(
      _event(i, bots[i % bots.length], 'Fixture conversation line $i.'),
    );
  }

  final library = Library(saver: MemorySaver());
  await library.load();
  final credentials = Credentials();
  final workspace = Directory.systemTemp.createTempSync(
    'orgbots-activity-profile-$pid-',
  );
  final slots = <RoomSlot>[];
  for (var i = 0; i < bots.length; i++) {
    final bot = library.addBot(
      BotDef(
        name: names[i],
        handle: 'agent$i',
        provider: ProviderKind.scripted,
        role: i == 0 ? 'coordinator' : 'engineer',
      ),
    );
    slots.add(RoomSlot(bot.id));
  }
  final room = library.addRoom(
    RoomDef(
      id: 'activity-preview',
      name: 'Activity profile',
      goal: 'Preview process and tool activity',
      path: workspace.path,
      maxTurns: 0,
      bots: slots,
    ),
  );
  library.sessions[room.id] = _PreviewRoom(
    room: room,
    library: library,
    sample: store,
  );

  runApp(
    MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: app.orgBotTheme(Brightness.light, desktop: true),
      darkTheme: app.orgBotTheme(Brightness.dark, desktop: true),
      themeMode: ThemeMode.system,
      home: OrgBotScreen(
        library: library,
        credentials: credentials,
        desktopClis: true,
      ),
    ),
  );
  WidgetsBinding.instance.addPostFrameCallback((_) {
    RoomStore.attentions.onRouteRequested?.call(
      AttentionTarget(
        roomId: room.id,
        reason: Attention.decision,
        eventId: 'activity-profile-open',
      ),
    );
  });
  // Let the native window settle and the production room route finish before
  // collecting timings, so the sample includes the real activity surface.
  await Future<void>.delayed(const Duration(seconds: 2));
  WidgetsBinding.instance.addTimingsCallback(onTimings);
  var sequence = 0;
  final burst = Timer.periodic(const Duration(milliseconds: 250), (_) {
    sequence++;
    final agent = bots[sequence % bots.length];
    link.emit(_stream(agent, {'type': 'thinking'}));
    for (var token = 0; token < 4; token++) {
      link.emit(
        _stream(agent, {
          'type': 'text_delta',
          'text': 'Fixture token $sequence-$token',
        }),
      );
    }
    if (sequence % 4 == 0) {
      final id = 'process-$sequence';
      link.emit(
        _stream(agent, {
          'type': 'tool_use_start',
          'id': id,
          'name': sequence.isEven ? 'Run command' : 'Edit files',
        }),
      );
      Timer(const Duration(milliseconds: 90), () {
        link.emit(
          _stream(agent, {'type': 'tool_use_end', 'id': id, 'succeeded': true}),
        );
      });
    }
  });

  await Future<void>.delayed(const Duration(seconds: 30));
  burst.cancel();
  WidgetsBinding.instance.removeTimingsCallback(onTimings);
  final buildSorted = timings.map((frame) => frame.build).toList()..sort();
  final rasterSorted = timings.map((frame) => frame.raster).toList()..sort();
  int percentile(List<int> values, double fraction) {
    if (values.isEmpty) return 0;
    final index = (values.length * fraction).floor().clamp(
      0,
      values.length - 1,
    );
    return values[index];
  }

  // This JSON line is consumed by the desktop profile harness runner.
  // ignore: avoid_print
  print(
    jsonEncode({
      'activity_profile_frames': timings.length,
      'build_p50_us': percentile(buildSorted, .50),
      'build_p95_us': percentile(buildSorted, .95),
      'build_max_us': buildSorted.isEmpty ? 0 : buildSorted.last,
      'raster_p50_us': percentile(rasterSorted, .50),
      'raster_p95_us': percentile(rasterSorted, .95),
      'raster_max_us': rasterSorted.isEmpty ? 0 : rasterSorted.last,
      'bots': bots.length,
      'seeded_messages': 500,
      'duration_seconds': 30,
    }),
  );

  // Keep the native window open after sampling so the owner can inspect and
  // interact with the seeded room and its question/process panels.
}
