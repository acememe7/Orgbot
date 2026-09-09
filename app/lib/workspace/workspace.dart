import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:dartssh2/dartssh2.dart';
import 'package:flutter/services.dart';

import '../orgbot/models.dart' show genId;

/// Stored only inside the encrypted library. Never put these secrets in TOML.
class WorkspaceConfig {
  String kind,
      path,
      host,
      username,
      password,
      privateKey,
      passphrase,
      fingerprint;
  int port;
  bool writable, commands;
  WorkspaceConfig({
    this.kind = 'phone',
    this.path = '',
    this.host = '',
    this.port = 22,
    this.username = '',
    this.password = '',
    this.privateKey = '',
    this.passphrase = '',
    this.fingerprint = '',
    this.writable = true,
    this.commands = false,
  });
  bool get isRemote => kind == 'ssh';
  String get label => isRemote ? '$username@$host:$port' : 'This phone';
  Map<String, Object?> toJson() => {
    'kind': kind,
    'path': path,
    'host': host,
    'port': port,
    'username': username,
    'password': password,
    'private_key': privateKey,
    'passphrase': passphrase,
    'fingerprint': fingerprint,
    'writable': writable,
    'commands': commands,
  };
  factory WorkspaceConfig.fromJson(Map<String, Object?> j) => WorkspaceConfig(
    kind: j['kind'] as String? ?? 'phone',
    path: j['path'] as String? ?? '',
    host: j['host'] as String? ?? '',
    port: j['port'] as int? ?? 22,
    username: j['username'] as String? ?? '',
    password: j['password'] as String? ?? '',
    privateKey: j['private_key'] as String? ?? '',
    passphrase: j['passphrase'] as String? ?? '',
    fingerprint: j['fingerprint'] as String? ?? '',
    writable: j['writable'] as bool? ?? true,
    commands: j['commands'] as bool? ?? false,
  );
  WorkspaceConfig copy() => WorkspaceConfig.fromJson(toJson());
}

typedef TrustHost =
    Future<bool> Function(String host, String keyType, String fingerprint);
const workspaceChannel = MethodChannel('orgbot/workspace');
const maxFileBytes = 1024 * 1024;

// Keep the isolate closure away from connection state (sockets and Futures).
Future<List<SSHKeyPair>> _parsePrivateKey(String pem, String passphrase) =>
    Isolate.run(
      () => SSHKeyPair.fromPem(pem, passphrase.isEmpty ? null : passphrase),
    );

/// The model's computer. Phone files and SSH use the same bounded tool contract.
class WorkspaceConnection {
  final WorkspaceConfig config;
  final String roomId;
  WorkspaceConnection(this.config, this.roomId);
  String root = '';
  SSHClient? _ssh;
  SftpClient? _sftp;
  SSHSession? _command;
  bool _closed = false;
  bool _disconnected = false;
  Future<void>? _connecting;
  bool _busy = false;
  int _epoch = 0;
  DateTime? _lastActivity;
  bool get connected =>
      !_closed &&
      !_disconnected &&
      root.isNotEmpty &&
      (!config.isRemote || (_ssh != null && !_ssh!.isClosed));

  static Future<String> phoneBase() async {
    if (Platform.isAndroid) {
      return (await workspaceChannel.invokeMethod<String>('directory'))!;
    }
    // Used by local tests; desktop room execution continues through system CLIs.
    return '${Directory.systemTemp.path}/orgbot-workspaces';
  }

  Future<void> connect({TrustHost? trust}) => _connecting ??= _connect(
    trust: trust,
  ).whenComplete(() => _connecting = null);

  Future<void> _connect({TrustHost? trust}) async {
    if (_closed) throw StateError('Connection closed; open a new connection.');
    if (connected) return;
    await cancelOperation();
    if (_closed) throw StateError('Connection cancelled.');
    final epoch = _epoch;
    final previousRoot = root;
    if (config.isRemote) {
      if (config.host.trim().isEmpty ||
          config.username.trim().isEmpty ||
          config.port < 1 ||
          config.port > 65535 ||
          !config.path.startsWith('/')) {
        throw const FormatException(
          'Enter a host, username, valid port and absolute remote folder.',
        );
      }
      if (config.password.isEmpty && config.privateKey.trim().isEmpty) {
        throw const FormatException('Enter an SSH password or private key.');
      }
      final pem = config.privateKey;
      final passphrase = config.passphrase;
      List<SSHKeyPair> keys;
      try {
        keys = pem.trim().isEmpty
            ? <SSHKeyPair>[]
            : await _parsePrivateKey(pem, passphrase);
      } catch (_) {
        throw const FormatException(
          'Could not read the private key. Import or paste the complete OpenSSH/PEM private key and check its passphrase.',
        );
      }
      if (_closed || epoch != _epoch) throw StateError('Connection cancelled.');
      final socket = await SSHSocket.connect(
        config.host.trim(),
        config.port,
        timeout: const Duration(seconds: 15),
      );
      if (_closed || epoch != _epoch) {
        await socket.close();
        throw StateError('Connection cancelled.');
      }
      String? refused;
      final ssh = _ssh = SSHClient(
        socket,
        username: config.username.trim(),
        identities: keys,
        handshakeTimeout: const Duration(seconds: 60),
        authTimeout: const Duration(seconds: 15),
        onPasswordRequest: () =>
            config.password.isEmpty ? null : config.password,
        onVerifyHostKey: (type, bytes) async {
          final fingerprint = utf8.decode(bytes);
          if (config.fingerprint.isNotEmpty) {
            final matches = config.fingerprint == fingerprint;
            if (!matches) {
              refused =
                  'Server identity changed. Verify its new fingerprint before trusting it in workspace settings.';
            }
            return matches;
          }
          final accepted =
              await trust?.call(config.label, type, fingerprint) ?? false;
          if (accepted && !_closed) config.fingerprint = fingerprint;
          if (!accepted) refused = 'Server identity was not trusted.';
          return accepted && !_closed;
        },
      );
      // Transport errors must be consumed as well as authentication errors.
      unawaited(
        ssh.done.then(
          (_) {
            if (identical(_ssh, ssh)) _disconnected = true;
          },
          onError: (Object _) {
            if (identical(_ssh, ssh)) _disconnected = true;
          },
        ),
      );
      try {
        await _ssh!.authenticated;
        _sftp = await _ssh!.sftp().timeout(const Duration(seconds: 15));
        final resolvedRoot = await _sftp!
            .absolute(config.path)
            .timeout(const Duration(seconds: 15));
        final attrs = await _sftp!
            .stat(resolvedRoot)
            .timeout(const Duration(seconds: 15));
        if (!attrs.isDirectory) {
          throw const FileSystemException(
            'Remote workspace is not a directory.',
          );
        }
        if (previousRoot.isNotEmpty && resolvedRoot != previousRoot) {
          _closed = true;
          throw StateError(
            'The remote folder changed. Reopen workspace settings before continuing.',
          );
        }
        if (ssh.isClosed || epoch != _epoch) {
          throw StateError('Workspace disconnected during connection.');
        }
        root = resolvedRoot;
        _disconnected = false;
        _lastActivity = DateTime.now();
      } catch (error) {
        await cancelOperation();
        if (refused != null) {
          _closed = true;
          throw StateError(refused!);
        }
        if (error is SSHAuthFailError) {
          _closed = true;
          throw StateError(
            keys.isEmpty
                ? 'SSH rejected the password login for ${config.username}. Check the username and password. If the server requires SSH keys, select Private key; a VPS console password does not enable SSH password login.'
                : 'SSH rejected the private key for ${config.username}. Check the username and that this key is authorized for that account on the server.',
          );
        }
        rethrow;
      }
    } else {
      final base = await phoneBase();
      final folder = config.path.trim();
      // Empty selects a separate, persistent folder for this room.
      final directory = Directory(folder.isEmpty ? '$base/$roomId' : folder);
      if (folder.isNotEmpty && !folder.startsWith('/')) {
        throw const FormatException(
          'A phone folder must be an absolute path, or leave it blank.',
        );
      }
      if (folder.isEmpty) await directory.create(recursive: true);
      root = await directory.resolveSymbolicLinks();
      _disconnected = false;
    }
    if (!connected) throw StateError('Workspace disconnected.');
  }

  Map<String, Object?> get description => {
    'label': config.label,
    'root': root,
    'writable': config.writable,
    'commands': config.isRemote && config.writable && config.commands,
  };

  Future<Object?> execute(String name, Map<String, Object?> args) async {
    if (_closed) {
      throw StateError('Workspace closed. Reconnect before continuing.');
    }
    if (_busy) throw StateError('A workspace operation is already running.');
    final read = name == 'workspace_read' || name == 'workspace_list';
    if (!read && !config.writable) throw StateError('Workspace is read-only.');
    if (name == 'workspace_run' && (!config.isRemote || !config.commands)) {
      throw StateError('Shell commands are disabled for this workspace.');
    }
    _busy = true;
    try {
      // Probe an idle transport before sending an operation, including writes.
      // SSH's built-in keepalive sends pings but does not time out unanswered pings.
      if (config.isRemote &&
          connected &&
          _lastActivity != null &&
          DateTime.now().difference(_lastActivity!) >
              const Duration(seconds: 15)) {
        try {
          await _ssh!.ping().timeout(const Duration(seconds: 5));
        } on TimeoutException {
          await cancelOperation();
        } on SSHError {
          await cancelOperation();
        } on SocketException {
          await cancelOperation();
        }
      }
      for (var attempt = 0; ; attempt++) {
        if (!connected && config.isRemote && config.fingerprint.isNotEmpty) {
          await connect();
        }
        if (!connected) {
          throw StateError(
            'Workspace disconnected. Reconnect before continuing.',
          );
        }
        try {
          final result = await _execute(name, args).timeout(
            Duration(
              seconds: name == 'workspace_run'
                  ? commandTimeoutSeconds(args) + 20
                  : 30,
            ),
          );
          _lastActivity = DateTime.now();
          return result;
        } catch (error) {
          if (error is FormatException) rethrow;
          final transportLost =
              error is TimeoutException ||
              error is SSHSocketError ||
              error is SSHStateError ||
              error is SftpAbortError ||
              error is SocketException ||
              !connected;
          if (!transportLost) rethrow;
          await cancelOperation();
          // Reads can be retried once after reconnecting to the pinned host.
          // A write or command may already have taken effect: never replay it.
          if (read && config.isRemote && attempt == 0 && !_closed) continue;
          throw StateError(
            'Workspace connection interrupted. Reads reconnect automatically. '
            'A write or command may have partially completed; inspect its effects before retrying.',
          );
        }
      }
    } finally {
      _busy = false;
    }
  }

  Future<Object?> importAttachment(File source, String filename) async {
    final file = await source.open();
    late Uint8List bytes;
    try {
      bytes = await file.read(20 * 1024 * 1024 + 1);
    } finally {
      await file.close();
    }
    if (bytes.length > 20 * 1024 * 1024) {
      throw StateError('Attachment exceeds 20 MiB.');
    }
    final safeName = filename.replaceAll(RegExp(r'[^a-zA-Z0-9._-]'), '_');
    final path =
        '${genId('attachment')}_${safeName.substring(0, safeName.length.clamp(0, 100))}';
    return execute('workspace_import_attachment', {
      'path': path,
      '_bytes': bytes,
    });
  }

  Future<Object?> _execute(String name, Map<String, Object?> args) async {
    final epoch = _epoch;
    if (name == 'workspace_run') {
      return _run(
        requiredString(args, 'command'),
        commandTimeoutSeconds(args),
        epoch,
      );
    }
    if (![
      'workspace_list',
      'workspace_read',
      'workspace_write',
      'workspace_mkdir',
      'workspace_import_attachment',
    ].contains(name)) {
      throw StateError('Unknown workspace tool.');
    }
    final relative = cleanPath(requiredString(args, 'path'));
    final creating =
        name == 'workspace_write' ||
        name == 'workspace_mkdir' ||
        name == 'workspace_import_attachment';
    final path = await _resolve(relative, parent: creating);
    _checkOperation(epoch);
    final sftp = _sftp;
    if (name == 'workspace_list') {
      final entries = <Map<String, Object?>>[];
      if (sftp != null) {
        // Closing on an oversized directory also releases its remote handle.
        await for (final batch in sftp.readdir(path)) {
          for (final entry in batch) {
            if (entry.filename == '.' || entry.filename == '..') continue;
            if (entries.length >= 500) {
              await cancelOperation();
              throw const FormatException(
                'Directory exceeds 500 entries. Choose a smaller folder.',
              );
            }
            entries.add({
              'name': entry.filename,
              'directory': entry.attr.isDirectory,
              'size': entry.attr.size,
            });
          }
        }
      } else {
        await for (final entry in Directory(path).list(followLinks: false)) {
          if (entries.length >= 500) {
            throw StateError(
              'Directory exceeds 500 entries. Choose a smaller folder.',
            );
          }
          entries.add({
            'name': entry.path.split('/').last,
            'directory': entry is Directory,
          });
        }
      }
      return entries;
    }
    if (name == 'workspace_read') {
      Uint8List bytes;
      if (sftp != null) {
        final file = await sftp.open(path);
        try {
          if (((await file.stat()).size ?? 0) > maxFileBytes) {
            throw StateError('File exceeds 1 MiB.');
          }
          bytes = await file.readBytes(length: maxFileBytes + 1);
        } finally {
          await file.close();
        }
      } else {
        final file = await File(path).open();
        try {
          bytes = await file.read(maxFileBytes + 1);
        } finally {
          await file.close();
        }
      }
      if (bytes.length > maxFileBytes) throw StateError('File exceeds 1 MiB.');
      return utf8.decode(bytes);
    }
    if (relative == '.') throw StateError('Cannot replace the workspace root.');
    if (name == 'workspace_mkdir') {
      if (sftp != null) {
        await sftp.mkdir(path);
      } else {
        await Directory(path).create();
      }
      return {'created': relative};
    }
    final bytes = name == 'workspace_import_attachment'
        ? args['_bytes'] as Uint8List
        : utf8.encode(requiredString(args, 'content'));
    if (bytes.length >
        (name == 'workspace_import_attachment'
            ? 20 * 1024 * 1024
            : maxFileBytes)) {
      throw StateError('File exceeds its size limit.');
    }
    // Rename a new sibling over the destination; never truncate the original.
    final temp = '$path.${genId('orgbot-tmp')}';
    try {
      if (sftp != null) {
        var permissions = 0x180; // 0600 for new files.
        try {
          final attrs = await sftp.stat(path, followLink: false);
          if (attrs.isFile) permissions = attrs.mode!.value & 0x1ff;
        } on SftpStatusError catch (error) {
          if (error.code != SftpStatusCode.noSuchFile) rethrow;
        }
        final file = await sftp.open(
          temp,
          mode:
              SftpFileOpenMode.write |
              SftpFileOpenMode.create |
              SftpFileOpenMode.exclusive,
        );
        try {
          // Set mode before content is written, including private replacements.
          await file.setStat(
            SftpFileAttrs(mode: SftpFileMode.value(permissions)),
          );
          await file.writeBytes(bytes);
        } finally {
          await file.close();
        }
        _checkOperation(epoch);
        await sftp.rename(temp, path);
      } else {
        final permissions =
            await FileSystemEntity.type(path, followLinks: false) ==
                FileSystemEntityType.file
            ? (await File(path).stat()).mode & 0x1ff
            : 0x180;
        await File(temp).create(exclusive: true);
        if (Platform.isAndroid) {
          await workspaceChannel.invokeMethod<void>('chmod', {
            'path': temp,
            'mode': permissions,
          });
        } else {
          final changed = await Process.run('chmod', [
            permissions.toRadixString(8),
            temp,
          ]);
          if (changed.exitCode != 0) {
            throw FileSystemException(
              'Could not preserve file permissions.',
              temp,
            );
          }
        }
        await File(temp).writeAsBytes(bytes, flush: true);
        _checkOperation(epoch);
        await File(temp).rename(path);
      }
    } finally {
      try {
        if (sftp != null) {
          await sftp.remove(temp).timeout(const Duration(seconds: 2));
        } else if (await File(temp).exists()) {
          await File(temp).delete();
        }
      } catch (_) {
        /* Never mask a failed write; the original is preserved. */
      }
    }
    return {'written': relative, 'bytes': bytes.length};
  }

  Future<String> _resolve(String relative, {bool parent = false}) async {
    final path = relative == '.' ? root : '$root/$relative';
    final parts = path.split('/');
    final target = parent ? (parts..removeLast()).join('/') : path;
    final canonical = _sftp != null
        ? await _sftp!.absolute(target.isEmpty ? '/' : target)
        : await File(target).resolveSymbolicLinks();
    if (canonical != root &&
        !(canonical.startsWith(root == '/' ? '/' : '$root/'))) {
      throw StateError('Path escapes the selected workspace.');
    }
    // Replacing a final symlink via rename is safe; reads resolve its target.
    return parent ? '$canonical/${path.split('/').last}' : canonical;
  }

  Future<Object?> _run(String command, int timeoutSeconds, int epoch) async {
    if (command.trim().isEmpty ||
        command.length > 16384 ||
        command.contains('\u0000')) {
      throw const FormatException('Command is empty or too long.');
    }
    final session = await _ssh!.execute(
      'cd -- ${shellQuote(root)} && exec sh -c ${shellQuote(command)}',
    );
    if (epoch != _epoch || !connected) {
      _stopCommand(session);
      throw StateError(
        'Command connection was cancelled before it opened. Inspect before retrying.',
      );
    }
    _command = session;
    final output = BytesBuilder(copy: false);
    var truncated = false;
    void collect(Uint8List bytes) {
      final remaining = 65536 - output.length;
      if (remaining > 0) {
        output.add(
          bytes.length <= remaining ? bytes : bytes.sublist(0, remaining),
        );
      }
      if (bytes.length > remaining) truncated = true;
    }

    final stdout = session.stdout.listen(collect);
    final stderr = session.stderr.listen(collect);
    try {
      await Future.wait([
        session.done,
        stdout.asFuture<void>(),
        stderr.asFuture<void>(),
      ]).timeout(Duration(seconds: timeoutSeconds));
      if (session.exitCode == null) {
        throw StateError(
          'Command connection ended without an exit status. Inspect its effects before retrying.',
        );
      }
      return {
        'exit_code': session.exitCode,
        'output': utf8.decode(output.takeBytes(), allowMalformed: true),
        'truncated': truncated,
      };
    } on TimeoutException {
      // Stop only this command channel. SFTP and the authenticated SSH transport
      // remain usable for inspecting partial output or other work.
      throw StateError(
        'Command reached its $timeoutSeconds-second limit; its channel was closed. '
        'SSH remains available. Changes or child processes may remain; inspect before retrying. '
        'For a longer command, set timeout_seconds (maximum 1800).',
      );
    } finally {
      _stopCommand(session);
      await stdout.cancel();
      await stderr.cancel();
      if (identical(_command, session)) _command = null;
    }
  }

  void _checkOperation(int epoch) {
    if (epoch != _epoch || !connected) {
      throw StateError('Workspace operation was cancelled.');
    }
  }

  void _stopCommand(SSHSession? session) {
    if (session == null) return;
    try {
      if (session.exitCode == null) session.kill(SSHSignal.KILL);
    } on SSHError {
      /* The peer already closed the channel or transport. */
    }
    try {
      session.close();
    } on SSHError {
      /* Already disconnected. */
    }
  }

  /// Closes the transport on pause/cancel so delayed writes cannot be retried.
  Future<void> close() async {
    _closed = true;
    await cancelOperation();
  }

  /// Cancel the current transport; the next operation can open a pinned connection.
  Future<void> cancelOperation() async {
    _epoch++;
    _disconnected = true;
    _stopCommand(_command);
    final ssh = _ssh;
    _ssh = null;
    _sftp = null;
    if (ssh != null) {
      try {
        await ssh.close().timeout(const Duration(seconds: 2));
      } catch (_) {}
    }
  }
}

String requiredString(Map<String, Object?> args, String key) {
  final value = args[key];
  if (value is! String || value.contains('\u0000')) {
    throw FormatException('$key must be text without NUL bytes.');
  }
  return value;
}

String cleanPath(String path) {
  if (path.isEmpty || path == '.') return '.';
  if (path.startsWith('/') ||
      path.contains('\\') ||
      path.contains('\u0000') ||
      path.length > 4096) {
    throw const FormatException('Use a relative workspace path.');
  }
  final parts = path
      .split('/')
      .where((part) => part.isNotEmpty && part != '.')
      .toList();
  if (parts.contains('..')) {
    throw const FormatException('Parent traversal is not allowed.');
  }
  return parts.isEmpty ? '.' : parts.join('/');
}

String shellQuote(String text) => "'${text.replaceAll("'", "'\"'\"'")}'";

/// Shared command bound for the shell channel and its outer operation deadline.
int commandTimeoutSeconds(Map<String, Object?> args) {
  final seconds = args['timeout_seconds'] ?? 300;
  if (seconds is! int || seconds < 1 || seconds > 1800) {
    throw const FormatException(
      'timeout_seconds must be an integer from 1 to 1800.',
    );
  }
  return seconds;
}
