import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

typedef TrashProcessRunner =
    Future<ProcessResult> Function(String executable, List<String> arguments);

const _trashHelperTimeout = Duration(seconds: 30);
const _maxHelperOutputBytes = 64 * 1024;

class TrashSettings {
  const TrashSettings({
    required this.root,
    required this.source,
    required this.requiresSelection,
  });

  final String root;
  final String source;
  final bool requiresSelection;

  factory TrashSettings.fromJson(Object? value) {
    if (value is! Map<String, dynamic> ||
        value['root'] is! String ||
        value['source'] is! String ||
        value['requires_selection'] is! bool) {
      throw const FormatException(
        'The trash helper returned an incomplete settings response.',
      );
    }
    final root = value['root'] as String;
    final source = value['source'] as String;
    if (!root.startsWith('/') || source.trim().isEmpty) {
      throw const FormatException(
        'The trash helper returned an invalid storage location.',
      );
    }
    return TrashSettings(
      root: root,
      source: source,
      requiresSelection: value['requires_selection'] as bool,
    );
  }
}

/// Calls the trusted trash helper without a shell and validates its JSON.
class TrashSettingsClient {
  const TrashSettingsClient({
    this.processRunner,
    this.helperPath,
    this.timeout = _trashHelperTimeout,
  });

  final TrashProcessRunner? processRunner;
  final String? helperPath;
  final Duration timeout;

  Future<TrashSettings> read() => _invoke(const ['config']);

  Future<TrashSettings> configure(String root) {
    final path = root.trim();
    if (!path.startsWith('/') || path.contains('\n') || path.contains('\r')) {
      throw const FormatException('Enter an absolute folder path.');
    }
    return _invoke(['configure', '--root', path]);
  }

  Future<TrashSettings> _invoke(List<String> command) async {
    final helper = await _resolveHelper();
    if (helper == null) {
      throw const FormatException(
        'File recovery is unavailable because the trash helper could not be found.',
      );
    }
    final executable = processRunner == null ? '/usr/bin/python3' : 'python3';
    final arguments = [helper, ...command];
    final runner = processRunner;
    final result = runner == null
        ? await _runBounded(executable, arguments)
        : await runner(executable, arguments).timeout(
            timeout,
            onTimeout: () => throw const FormatException(
              'The trash helper timed out. Retry, or check whether the recovery drive is available.',
            ),
          );
    if (result.exitCode != 0) {
      final detail = result.stderr is String
          ? (result.stderr as String).trim()
          : '';
      throw FormatException(
        detail.isEmpty
            ? 'The trash helper failed (exit ${result.exitCode}).'
            : detail,
      );
    }
    final decoded = jsonDecode(
      result.stdout is String ? result.stdout as String : '',
    );
    final settings = TrashSettings.fromJson(decoded);
    if (command.first == 'configure' && settings.requiresSelection) {
      throw const FormatException(
        'The trash helper did not confirm the new storage folder.',
      );
    }
    return settings;
  }

  Future<ProcessResult> _runBounded(
    String executable,
    List<String> arguments,
  ) async {
    final process = await Process.start(executable, arguments);
    final stdout = BytesBuilder(copy: false);
    final stderr = BytesBuilder(copy: false);
    var outputTooLarge = false;

    var streamsRemaining = 2;
    final streamsDone = Completer<void>();
    Object? streamError;
    void markStreamDone() {
      streamsRemaining--;
      if (streamsRemaining == 0 && !streamsDone.isCompleted) {
        streamsDone.complete();
      }
    }

    StreamSubscription<List<int>> collect(
      Stream<List<int>> stream,
      BytesBuilder output,
    ) => stream.listen(
      (chunk) {
        if (outputTooLarge) return;
        if (output.length + chunk.length > _maxHelperOutputBytes) {
          outputTooLarge = true;
          process.kill(ProcessSignal.sigkill);
          return;
        }
        output.add(chunk);
      },
      onError: (Object error) {
        streamError ??= error;
        process.kill(ProcessSignal.sigkill);
      },
      onDone: markStreamDone,
    );

    final stdoutSubscription = collect(process.stdout, stdout);
    final stderrSubscription = collect(process.stderr, stderr);
    final exitCodeFuture = process.exitCode;
    try {
      final exitCode = await exitCodeFuture.timeout(
        timeout,
        onTimeout: () {
          process.kill(ProcessSignal.sigkill);
          throw const FormatException(
            'The trash helper timed out. Retry, or check whether the recovery drive is available.',
          );
        },
      );
      await streamsDone.future.timeout(const Duration(seconds: 2));
      if (streamError != null) {
        throw const FormatException(
          'Could not read the trash helper response.',
        );
      }
      if (outputTooLarge) {
        throw const FormatException(
          'The trash helper returned too much output to process safely.',
        );
      }
      return ProcessResult(
        process.pid,
        exitCode,
        utf8.decode(stdout.takeBytes(), allowMalformed: true),
        utf8.decode(stderr.takeBytes(), allowMalformed: true),
      );
    } catch (_) {
      process.kill(ProcessSignal.sigkill);
      await Future.wait<void>([
        exitCodeFuture.then<void>((_) {}, onError: (Object _) {}),
        stdoutSubscription.cancel(),
        stderrSubscription.cancel(),
      ]).timeout(const Duration(seconds: 2), onTimeout: () async => <void>[]);
      rethrow;
    }
  }

  Future<String?> _resolveHelper() async {
    if (helperPath != null) return helperPath;
    final executableSibling =
        '${File(Platform.resolvedExecutable).parent.path}/orgbot-trash.py';
    if (await _isTrustedHelper(executableSibling)) return executableSibling;

    final workingDirectory = Directory.current.path;
    for (final candidate in [
      '$workingDirectory/app/packaging/orgbot-trash.py',
      '$workingDirectory/packaging/orgbot-trash.py',
    ]) {
      if (await _isTrustedHelper(candidate)) return candidate;
    }

    const systemHelper = '/usr/lib/orgbot/orgbot-trash.py';
    if (await _isTrustedHelper(systemHelper)) return systemHelper;
    return null;
  }

  Future<bool> _isTrustedHelper(String path) async {
    const check =
        'import os,stat,sys; i=os.lstat(sys.argv[1]); print("trusted" if stat.S_ISREG(i.st_mode) and i.st_uid in (0,os.getuid()) and not i.st_mode & 0o022 else "untrusted")';
    try {
      final result = await _runBounded('/usr/bin/python3', ['-c', check, path]);
      return result.exitCode == 0 && result.stdout.trim() == 'trusted';
    } on Object {
      return false;
    }
  }
}
