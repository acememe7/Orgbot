// The engine, running inside this process.
//
// On a phone there is no subprocess to spawn, so the room runs through the
// Rust engine compiled into liborgbot_ffi.so and loaded with dart:ffi. This is
// the same engine the desktop runs — not a reimplementation — so a room
// behaves identically on both.
//
// The C side is deliberately dull: strings in, JSON out, and a poll we drain.
// Nothing calls back into Dart, because a callback arriving on a Rust worker
// thread has to be marshalled to the right isolate and getting that wrong is a
// crash rather than a bug.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

// ---- C signatures -----------------------------------------------------------

typedef _OpenNative = Uint64 Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _OpenDart = int Function(Pointer<Utf8>, Pointer<Utf8>);

typedef _PollNative = Pointer<Utf8> Function(Uint64);
typedef _PollDart = Pointer<Utf8> Function(int);

typedef _SayNative = Int32 Function(Uint64, Pointer<Utf8>, Pointer<Utf8>);
typedef _SayDart = int Function(int, Pointer<Utf8>, Pointer<Utf8>);

typedef _HandleNative = Int32 Function(Uint64);
typedef _HandleDart = int Function(int);

typedef _CloseNative = Void Function(Uint64);
typedef _CloseDart = void Function(int);

typedef _FreeNative = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

typedef _VersionNative = Uint32 Function();
typedef _VersionDart = int Function();

/// The ABI this binding is written against.
const int kExpectedAbi = 1;

/// Thrown when the engine library cannot be used.
class EmbeddedEngineError implements Exception {
  final String message;
  EmbeddedEngineError(this.message);
  @override
  String toString() => message;
}

/// The loaded engine library.
class EmbeddedEngine {
  EmbeddedEngine._(DynamicLibrary lib)
      : _open = lib.lookupFunction<_OpenNative, _OpenDart>('orgbot_room_open'),
        _poll = lib.lookupFunction<_PollNative, _PollDart>('orgbot_room_poll'),
        _snapshot =
            lib.lookupFunction<_PollNative, _PollDart>('orgbot_room_snapshot'),
        _say = lib.lookupFunction<_SayNative, _SayDart>('orgbot_room_say'),
        _pause =
            lib.lookupFunction<_HandleNative, _HandleDart>('orgbot_room_pause'),
        _resume = lib
            .lookupFunction<_HandleNative, _HandleDart>('orgbot_room_resume'),
        _close =
            lib.lookupFunction<_CloseNative, _CloseDart>('orgbot_room_close'),
        _lastError = lib.lookupFunction<Pointer<Utf8> Function(),
            Pointer<Utf8> Function()>('orgbot_last_error'),
        _free = lib.lookupFunction<_FreeNative, _FreeDart>('orgbot_string_free'),
        _abi = lib
            .lookupFunction<_VersionNative, _VersionDart>('orgbot_abi_version');

  final _OpenDart _open;
  final _PollDart _poll;
  final _PollDart _snapshot;
  final _SayDart _say;
  final _HandleDart _pause;
  final _HandleDart _resume;
  final _CloseDart _close;
  final Pointer<Utf8> Function() _lastError;
  final _FreeDart _free;
  final _VersionDart _abi;

  static EmbeddedEngine? _instance;

  /// Whether this platform can run the engine in-process.
  static bool get isSupported =>
      Platform.isAndroid || Platform.isLinux || Platform.isMacOS;

  /// Load the engine, or throw with a readable reason.
  static EmbeddedEngine load() {
    final existing = _instance;
    if (existing != null) return existing;

    final library = _openLibrary();
    final engine = EmbeddedEngine._(library);
    final abi = engine._abi();
    if (abi != kExpectedAbi) {
      throw EmbeddedEngineError(
          'the engine library speaks ABI $abi, this app expects $kExpectedAbi');
    }
    _instance = engine;
    return engine;
  }

  static DynamicLibrary _openLibrary() {
    // On Android the .so is packed into the APK and resolved by name; on
    // desktop it sits next to the executable or in the build tree.
    final candidates = <String>[
      'liborgbot_ffi.so',
      if (Platform.isLinux || Platform.isMacOS) ...[
        File(Platform.resolvedExecutable).parent.path + '/lib/liborgbot_ffi.so',
        File(Platform.resolvedExecutable).parent.path + '/liborgbot_ffi.so',
        'core/target/release/liborgbot_ffi.so',
        '../core/target/release/liborgbot_ffi.so',
      ],
    ];
    Object? lastError;
    for (final candidate in candidates) {
      try {
        return DynamicLibrary.open(candidate);
      } catch (e) {
        lastError = e;
      }
    }
    throw EmbeddedEngineError(
        'could not load the engine library (liborgbot_ffi.so): $lastError');
  }

  /// The last error the library recorded, if any.
  String? takeError() {
    final ptr = _lastError();
    if (ptr == nullptr) return null;
    final text = ptr.toDartString();
    _free(ptr);
    return text;
  }

  /// Start a room from a TOML room file.
  ///
  /// `environment` carries credentials as `KEY=value`, so a key or token never
  /// has to be written into the room file.
  int openRoom(String toml, Map<String, String> environment) {
    final env = environment.entries.map((e) => '${e.key}=${e.value}').join('\n');
    final tomlPtr = toml.toNativeUtf8();
    final envPtr = env.toNativeUtf8();
    try {
      final handle = _open(tomlPtr, envPtr);
      if (handle == 0) {
        throw EmbeddedEngineError(takeError() ?? 'the room would not open');
      }
      return handle;
    } finally {
      calloc.free(tomlPtr);
      calloc.free(envPtr);
    }
  }

  /// Everything the room has broadcast since the last poll.
  List<Map<String, Object?>> poll(int handle) => _decodeList(_poll(handle));

  /// The room's current state, or null if it has stopped.
  Map<String, Object?>? snapshot(int handle) {
    final decoded = _decode(_snapshot(handle));
    return decoded;
  }

  /// Say something to the room as a person.
  bool say(int handle, String author, String body) {
    final a = author.toNativeUtf8();
    final b = body.toNativeUtf8();
    try {
      return _say(handle, a, b) == 1;
    } finally {
      calloc.free(a);
      calloc.free(b);
    }
  }

  /// Pause the room.
  bool pause(int handle) => _pause(handle) == 1;

  /// Resume a paused room.
  bool resume(int handle) => _resume(handle) == 1;

  /// Stop a room and release it.
  void closeRoom(int handle) => _close(handle);

  /// Read an owned string back, freeing it.
  String? _takeString(Pointer<Utf8> ptr) {
    if (ptr == nullptr) return null;
    final text = ptr.toDartString();
    _free(ptr);
    return text;
  }

  Map<String, Object?>? _decode(Pointer<Utf8> ptr) {
    final text = _takeString(ptr);
    if (text == null || text.isEmpty) return null;
    final value = jsonDecode(text);
    return value is Map<String, Object?> ? value : null;
  }

  List<Map<String, Object?>> _decodeList(Pointer<Utf8> ptr) {
    final text = _takeString(ptr);
    if (text == null || text.isEmpty) return const [];
    final value = jsonDecode(text);
    if (value is! List) return const [];
    return value.whereType<Map>().map((m) => m.cast<String, Object?>()).toList();
  }
}
