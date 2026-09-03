// The controller a phone screen binds to: it owns an engine + runner, runs
// the room off the UI thread, and notifies listeners as the log and board
// change. Everything here is plain Dart so it is testable without widgets.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../engine/engine_core.dart';
import '../engine/runner.dart';

enum LocalRunState { idle, running, paused, finished, failed }

/// Drives a local room and exposes a live snapshot for the UI.
class LocalRoomController extends ChangeNotifier {
  final Engine engine;
  final Runner runner;
  LocalRunState state = LocalRunState.idle;
  String? outcome;
  String? error;
  Timer? _ticker;
  bool _working = false;

  /// How the person is shown in the transcript.
  String speakerName = 'you';

  LocalRoomController({required this.engine, required this.runner});

  /// A default room: coordinator, a writer and an inspector.
  factory LocalRoomController.demo(String goal,
      {required BotProvider Function(String handle, List<String> roles) providerFor}) {
    final engine = Engine(name: 'room', goal: goal);
    final runner = Runner(engine);
    final c = LocalRoomController(engine: engine, runner: runner);
    c.seat('lead', const ['coordinator'], providerFor('lead', const ['coordinator']),
        mayCreate: true, maySubmit: true);
    c.seat('writer', const ['writer'], providerFor('writer', const ['writer']),
        maySubmit: true);
    c.seat('qa', const ['reviewer'], providerFor('qa', const ['reviewer']),
        mayInspect: true);
    return c;
  }

  void seat(String handle, List<String> roles, BotProvider provider,
      {bool mayCreate = false, bool mayInspect = false, bool maySubmit = false}) {
    runner.seat(handle, roles, provider,
        mayCreate: mayCreate, mayInspect: mayInspect, maySubmit: maySubmit);
  }

  /// Whether the room is resting and would answer if spoken to.
  bool get isAwaitingYou =>
      state == LocalRunState.idle || state == LocalRunState.finished;

  /// A compact snapshot for the UI.
  ({List<String> log, List<String> tasks, List<String> chat, String phase}) snapshot() {
    return (
      log: List.of(engine.log),
      tasks: engine.tasks.values
          .map((t) => t.state.name +
              ' ' +
              t.id +
              ' - ' +
              t.title +
              (t.assignee != null ? ' @' + engine.agents[t.assignee]!.handle : ''))
          .toList(),
      chat: engine.messages.map((m) => m.author + ': ' + m.body).toList(),
      phase: engine.phase.name,
    );
  }

  /// Drive the room until it stops needing to act.
  ///
  /// Re-entrant on purpose: pausing, resuming, and speaking all come back
  /// through here. The old version guarded on a one-shot `_runStarted` flag,
  /// which meant a paused room could never be restarted and a room that went
  /// idle was dead for good — the loop had returned and nothing called it
  /// again.
  Future<LocalRunState> _drive() async {
    if (_working) return state;
    _working = true;
    state = LocalRunState.running;
    notifyListeners();

    _ticker ??= Timer.periodic(const Duration(milliseconds: 350), (_) {
      notifyListeners();
    });

    try {
      final result = await runner.run();
      outcome = result.name;
      state = switch (result) {
        RunOutcome.paused => LocalRunState.paused,
        // Idle is a resting state, not an ending: the bots have nothing to do
        // until the person says something. The room stays usable.
        RunOutcome.idle => LocalRunState.idle,
        _ => LocalRunState.finished,
      };
    } catch (e) {
      error = e.toString();
      state = LocalRunState.failed;
    } finally {
      _ticker?.cancel();
      _ticker = null;
      _working = false;
      notifyListeners();
    }
    return state;
  }

  /// Start the room.
  Future<LocalRunState> start({bool manual = false}) => _drive();

  /// Say something to the room, and wake it to answer.
  ///
  /// Resets the idle counter so a room that had settled does not immediately
  /// declare itself stalled on the strength of what it did before you spoke.
  Future<void> say(String text) async {
    final body = text.trim();
    if (body.isEmpty) return;
    engine.postHuman(speakerName, body);
    runner.idleRounds = 0;
    if (engine.phase == RoomPhase.paused) engine.resume();
    if (engine.phase == RoomPhase.completed) return;
    notifyListeners();
    await _drive();
  }

  void pause() {
    engine.pause();
    state = LocalRunState.paused;
    notifyListeners();
  }

  void resume() {
    engine.resume();
    runner.idleRounds = 0;
    state = LocalRunState.running;
    notifyListeners();
    // Restart the loop: `run()` returned when the room paused, so without
    // this the room would sit in "running" forever without taking a turn.
    unawaited(_drive());
  }
}
