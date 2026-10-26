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
import 'dart:convert';
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
      null,
      'Desktop rooms use this executable. Android rooms use the embedded engine.',
    );
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
  return const EngineBinary(
    null,
    'The orgbot engine was not found. Set ORGBOT_ENGINE to its path, or install it alongside the app.',
  );
}

/// A running engine process hosting one room.
class CoreServer {
  final Process? process;

  /// The WebSocket URL the app should connect to.
  final String url;
  final String token;

  /// The generated room file, kept so it can be shown or removed.
  final File roomFile;

  /// Everything the engine has written to stderr, for diagnosis.
  final StringBuffer diagnostics;
  final String? _serviceDirectory;
  final String? _enginePath;

  CoreServer._(
    this.process,
    this.url,
    this.token,
    this.roomFile,
    this.diagnostics,
  ) : _serviceDirectory = null,
      _enginePath = null;

  CoreServer._service(
    this.url,
    this.token,
    this.roomFile,
    this.diagnostics,
    this._serviceDirectory,
    this._enginePath,
  ) : process = null;

  /// Stop the engine only for an explicit owner stop.
  Future<void> stop() async {
    final directory = _serviceDirectory;
    if (directory != null) {
      final result = await Process.run(_enginePath!, [
        'room-service',
        'stop',
        '--directory',
        directory,
      ]);
      if (result.exitCode != 0) {
        throw EngineStartError('Could not stop the Linux room service.');
      }
      return;
    }
    final running = process;
    if (running == null) return;
    running.kill(ProcessSignal.sigterm);
    // Give it a moment to close its socket cleanly before insisting.
    try {
      await running.exitCode.timeout(const Duration(seconds: 2));
    } on TimeoutException {
      running.kill(ProcessSignal.sigkill);
      await running.exitCode;
    }
    try {
      if (await roomFile.parent.exists()) {
        await roomFile.parent.delete(recursive: true);
      }
    } catch (_) {
      // A leftover temp file is not worth failing a shutdown over.
    }
  }

  /// Close the UI connection while leaving a Linux room service running.
  Future<void> detach() async {
    if (_serviceDirectory != null) return;
    await stop();
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
Future<CoreServer> startRoom(
  RoomFile room, {
  String? enginePath,
  String? roomId,
}) async {
  final binary = enginePath != null
      ? EngineBinary(enginePath, null)
      : findEngine();
  if (!binary.available) {
    throw EngineStartError(binary.problem ?? 'no engine binary');
  }

  if (Platform.isLinux && roomId != null) {
    return _startLinuxService(room, roomId, binary.path!);
  }

  final dir = await Directory.systemTemp.createTemp('orgbot-room-');
  final file = File('${dir.path}/room.toml');
  await file.writeAsString(room.toml);

  // A port picked by the OS, so two rooms can run at once and a stale process
  // cannot collide with a fixed one.
  final port = await _freePort();
  final address = '127.0.0.1:$port';

  final random = Random.secure();
  final token = List.generate(
    32,
    (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
  ).join();
  final process = await Process.start(
    binary.path!,
    ['serve', file.path, '--addr', address],
    environment: {
      ...Platform.environment,
      ...room.environment,
      'NO_COLOR': '1',
      'ORGBOT_ROOM_TOKEN': token,
    },
  );

  final diagnostics = StringBuffer();
  process.stderr
      .transform(const SystemEncoding().decoder)
      .listen(diagnostics.write);
  process.stdout
      .transform(const SystemEncoding().decoder)
      .listen(diagnostics.write);

  // The engine may exit immediately on a bad room file; racing the health
  // check against that turns a silent hang into a readable error.
  var exited = false;
  unawaited(process.exitCode.then((_) => exited = true));

  final ready = await _waitForHealth('http://$address/health', () => exited);
  if (!ready) {
    process.kill(ProcessSignal.sigkill);
    await dir.delete(recursive: true);
    final why = diagnostics.toString().trim();
    throw EngineStartError(
      why.isEmpty
          ? 'the engine did not start listening on $address'
          : _lastLines(why, 4),
    );
  }

  return CoreServer._(process, 'ws://$address/ws', token, file, diagnostics);
}

Future<CoreServer> _startLinuxService(
  RoomFile room,
  String roomId,
  String enginePath,
) async {
  if (!RegExp(r'^[a-zA-Z0-9_-]+$').hasMatch(roomId)) {
    throw EngineStartError('Invalid room identifier.');
  }
  final dataHome = Platform.environment['XDG_DATA_HOME']?.trim();
  final home = dataHome == null || dataHome.isEmpty
      ? '${Platform.environment['HOME'] ?? '.'}/.local/share'
      : dataHome;
  final directory = Directory('$home/orgbot/.services/$roomId').absolute;
  await directory.create(recursive: true);
  await _setMode(directory.path, '700');
  final roomFile = File('${directory.path}/room.toml');
  final diagnostics = StringBuffer();

  final existing = await _serviceCommand(enginePath, [
    'status',
    '--directory',
    directory.path,
  ]);
  final current = _decodeServiceResult(existing, required: false);
  // An outdated service (the engine was updated underneath it) is restarted
  // by `start`, so it gets the current configuration as well.
  final alreadyRunning =
      current != null &&
      current['running'] == true &&
      current['outdated'] != true;
  if (!alreadyRunning) {
    final existingType = await FileSystemEntity.type(
      roomFile.path,
      followLinks: false,
    );
    if (existingType == FileSystemEntityType.link) {
      throw EngineStartError('The Linux room configuration path is unsafe.');
    }
    await roomFile.writeAsString(room.toml, flush: true);
    await _setMode(roomFile.path, '600');
  }
  final port = await _freePort();
  final address = '127.0.0.1:$port';
  final started = await _serviceCommand(
    enginePath,
    ['start', '--directory', directory.path, '--addr', address],
    input: utf8.encode(jsonEncode(room.environment)),
    allowFailure: false,
  );
  final currentStarted = _decodeServiceResult(started, required: true)!;
  final details = (currentStarted['descriptor'] as Map).cast<String, Object?>();
  final url = details['url'] as String;
  final token = details['token'] as String;
  final ready = await _waitForHealth(
    url.replaceFirst('ws://', 'http://').replaceFirst('/ws', '/health'),
    () => false,
  );
  if (!ready) {
    await Process.run(enginePath, [
      'room-service',
      'stop',
      '--directory',
      directory.path,
    ]);
    throw EngineStartError(
      'The Linux room service did not answer health checks.',
    );
  }
  return CoreServer._service(
    url,
    token,
    roomFile,
    diagnostics,
    directory.path,
    enginePath,
  );
}

Future<ProcessResult> _serviceCommand(
  String enginePath,
  List<String> arguments, {
  List<int>? input,
  bool allowFailure = true,
}) async {
  final process = await Process.start(enginePath, [
    'room-service',
    ...arguments,
  ]);
  final stdoutFuture = process.stdout.transform(utf8.decoder).join();
  final stderrFuture = process.stderr.transform(utf8.decoder).join();
  if (input != null) process.stdin.add(input);
  await process.stdin.close();
  final code = await process.exitCode;
  final stdout = await stdoutFuture;
  final stderr = await stderrFuture;
  if (!allowFailure && code != 0) {
    final details = stderr.trim();
    throw EngineStartError(
      details.isEmpty
          ? 'Could not start the Linux room service.'
          : _lastLines(details, 3),
    );
  }
  return ProcessResult(process.pid, code, stdout, stderr);
}

Map<String, Object?>? _decodeServiceResult(
  ProcessResult result, {
  required bool required,
}) {
  if (result.exitCode != 0) {
    if (required) {
      final details = result.stderr.toString().trim();
      throw EngineStartError(
        details.isEmpty
            ? 'Could not start the Linux room service.'
            : _lastLines(details, 3),
      );
    }
    return null;
  }
  try {
    final decoded = jsonDecode(result.stdout as String);
    return (decoded as Map).cast<String, Object?>();
  } catch (_) {
    if (required) {
      throw EngineStartError(
        'The Linux room service returned an invalid status.',
      );
    }
    return null;
  }
}

Future<void> _setMode(String path, String mode) async {
  final result = await Process.run('chmod', [mode, path]);
  if (result.exitCode != 0) {
    throw EngineStartError('Could not secure the Linux room service files.');
  }
}

/// Poll the engine's health endpoint until it answers or gives up.
Future<bool> _waitForHealth(String url, bool Function() hasExited) async {
  final client = HttpClient()
    ..connectionTimeout = const Duration(milliseconds: 400);
  try {
    for (var attempt = 0; attempt < 60; attempt++) {
      if (hasExited()) return false;
      try {
        final request = await client.getUrl(Uri.parse(url));
        final response = await request.close().timeout(
          const Duration(seconds: 1),
        );
        await response.drain<void>().timeout(const Duration(seconds: 1));
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
