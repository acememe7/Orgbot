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
  bool _runStarted = false;

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

  /// Run the room in the background until it finishes, pauses or fails.
  Future<LocalRunState> start({bool manual = false}) async {
    if (_working || _runStarted) return state;
    _working = true;
    _runStarted = true;
    state = LocalRunState.running;
    notifyListeners();

    _ticker = Timer.periodic(const Duration(milliseconds: 350), (_) {
      notifyListeners();
    });

    try {
      final result = await runner.run();
      outcome = result.name;
      state = result == RunOutcome.paused
          ? LocalRunState.paused
          : LocalRunState.finished;
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

  void pause() {
    engine.pause();
    state = LocalRunState.paused;
    notifyListeners();
  }

  void resume() {
    engine.resume();
    state = LocalRunState.running;
    notifyListeners();
  }
}
