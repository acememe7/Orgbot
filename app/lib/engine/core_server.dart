// Run a room on the real engine.
//
// The app used to run rooms on a cut-down Dart port that had no MCP, no
// memories and no skills. This spawns the actual `orgbot` binary instead —
// the same engine the CLI uses — and the app connects to it over the
// WebSocket it already speaks. A bot on a subscription backend therefore gets
// the board as MCP tools on top of its own memories, skills and MCP servers,
// because that is what the engine already does.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:async';
import 'dart:io';
import 'dart:math';

import '../orgbot/room_file.dart';

/// Where the engine binary was found, or why it was not.
class EngineBinary {
  final String? path;
  final String? problem;
  const EngineBinary(this.path, this.problem);
  bool get available => path != null;
}

/// Find the `orgbot` engine binary.
///
/// Checked in order of how deliberate each location is: an explicit override,
/// then next to this executable (how an installed build ships), then the
/// usual install dirs, then a development build tree.
EngineBinary findEngine() {
  if (!(Platform.isLinux || Platform.isMacOS || Platform.isWindows)) {
    return const EngineBinary(
        null, 'The engine runs on desktop only. On a phone, join a hosted room.');
  }
  final home = Platform.environment['HOME'] ?? '';
  final beside = File(Platform.resolvedExecutable).parent.path;
  final candidates = <String>[
    if ((Platform.environment['ORGBOT_ENGINE'] ?? '').isNotEmpty)
      Platform.environment['ORGBOT_ENGINE']!,
    '$beside/orgbot-engine',
    '$beside/orgbot',
    '/usr/lib/orgbot/orgbot-engine',
    '/usr/bin/orgbot-engine',
    if (home.isNotEmpty) '$home/.local/bin/orgbot-engine',
    for (final dir in (Platform.environment['PATH'] ?? '').split(':'))
      if (dir.isNotEmpty) '$dir/orgbot-engine',
    // A checkout being worked on.
    'core/target/release/orgbot',
    '../core/target/release/orgbot',
  ];
  for (final candidate in candidates) {
    final file = File(candidate);
    // The app's own launcher is called `orgbot` too; a directory match or the
    // running executable itself must not be mistaken for the engine.
    if (candidate == Platform.resolvedExecutable) continue;
    if (file.existsSync() && file.statSync().mode & 0x49 != 0) {
      return EngineBinary(file.absolute.path, null);
    }
  }
  return const EngineBinary(null,
      'The orgbot engine was not found. Set ORGBOT_ENGINE to its path, or install it alongside the app.');
}

/// A running engine process hosting one room.
class CoreServer {
  final Process process;

  /// The WebSocket URL the app should connect to.
  final String url;

  /// The generated room file, kept so it can be shown or removed.
  final File roomFile;

  /// Everything the engine has written to stderr, for diagnosis.
  final StringBuffer diagnostics;

  CoreServer._(this.process, this.url, this.roomFile, this.diagnostics);

  /// Stop the engine and clean up the generated file.
  Future<void> stop() async {
    process.kill(ProcessSignal.sigterm);
    // Give it a moment to close its socket cleanly before insisting.
    unawaited(Future<void>.delayed(const Duration(seconds: 2)).then((_) {
      process.kill(ProcessSignal.sigkill);
    }));
    try {
      if (await roomFile.exists()) await roomFile.delete();
    } catch (_) {
      // A leftover temp file is not worth failing a shutdown over.
    }
  }
}

/// Something went wrong starting the engine.
class EngineStartError implements Exception {
  final String message;
  EngineStartError(this.message);
  @override
  String toString() => message;
}

/// Start the engine on a generated room file and wait until it is serving.
Future<CoreServer> startRoom(RoomFile room, {String? enginePath}) async {
  final binary = enginePath != null
      ? EngineBinary(enginePath, null)
      : findEngine();
  if (!binary.available) {
    throw EngineStartError(binary.problem ?? 'no engine binary');
  }

  final dir = await Directory.systemTemp.createTemp('orgbot-room-');
  final file = File('${dir.path}/room.toml');
  await file.writeAsString(room.toml);

  // A port picked by the OS, so two rooms can run at once and a stale process
  // cannot collide with a fixed one.
  final port = await _freePort();
  final address = '127.0.0.1:$port';

  final process = await Process.start(
    binary.path!,
    ['serve', file.path, '--addr', address],
    environment: {
      ...Platform.environment,
      ...room.environment,
      'NO_COLOR': '1',
    },
  );

  final diagnostics = StringBuffer();
  process.stderr.transform(const SystemEncoding().decoder).listen(diagnostics.write);
  process.stdout.transform(const SystemEncoding().decoder).listen(diagnostics.write);

  // The engine may exit immediately on a bad room file; racing the health
  // check against that turns a silent hang into a readable error.
  var exited = false;
  unawaited(process.exitCode.then((_) => exited = true));

  final ready = await _waitForHealth('http://$address/health', () => exited);
  if (!ready) {
    process.kill(ProcessSignal.sigkill);
    final why = diagnostics.toString().trim();
    throw EngineStartError(why.isEmpty
        ? 'the engine did not start listening on $address'
        : _lastLines(why, 4));
  }

  return CoreServer._(process, 'ws://$address/ws', file, diagnostics);
}

/// Poll the engine's health endpoint until it answers or gives up.
Future<bool> _waitForHealth(String url, bool Function() hasExited) async {
  final client = HttpClient()..connectionTimeout = const Duration(milliseconds: 400);
  try {
    for (var attempt = 0; attempt < 60; attempt++) {
      if (hasExited()) return false;
      try {
        final request = await client.getUrl(Uri.parse(url));
        final response = await request.close();
        await response.drain<void>();
        if (response.statusCode == 200) return true;
      } catch (_) {
        // Not listening yet.
      }
      await Future<void>.delayed(const Duration(milliseconds: 150));
    }
    return false;
  } finally {
    client.close(force: true);
  }
}

/// Ask the OS for a free port by binding and releasing one.
Future<int> _freePort() async {
  final socket = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
  final port = socket.port;
  await socket.close();
  // A tiny race remains between closing and the engine binding; a random high
  // port would have the same exposure and this at least asks the OS.
  return port == 0 ? 8800 + Random().nextInt(400) : port;
}

String _lastLines(String text, int count) {
  final lines = text.trim().split('\n');
  return lines.sublist(max(0, lines.length - count)).join('\n');
}
