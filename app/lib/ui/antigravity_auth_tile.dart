import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'workspace_helper.dart';

typedef AgyProcessRunner =
    Future<ProcessResult> Function(String executable, List<String> arguments);

typedef AgyTerminalLauncher =
    Future<void> Function(String executable, List<String> arguments);

const _pythonExecutable = '/usr/bin/python3';

class AntigravityAuthTile extends StatefulWidget {
  const AntigravityAuthTile({
    super.key,
    this.helperPath,
    this.processRunner,
    this.terminalLauncher,
  });

  final String? helperPath;
  final AgyProcessRunner? processRunner;
  final AgyTerminalLauncher? terminalLauncher;

  @override
  State<AntigravityAuthTile> createState() => _AntigravityAuthTileState();
}

class _AntigravityAuthTileState extends State<AntigravityAuthTile> {
  bool _busy = false;
  bool? _credentialsPresent;
  String? _statusError;

  Future<List<String>> _helperArguments(String action) async {
    final helper = await locateWorkspaceHelper(override: widget.helperPath);
    if (helper == null) {
      throw const FormatException(
        'The Antigravity sign-in helper is unavailable.',
      );
    }
    return [helper, 'agy-auth', action];
  }

  Future<Map<String, Object?>> _invokeStatus() async {
    final arguments = await _helperArguments('status');
    final result = await (widget.processRunner ?? _runProcess)(
      _pythonExecutable,
      arguments,
    );
    if (result.exitCode != 0 || result.stdout is! String) {
      throw const FormatException('Could not read Antigravity sign-in status.');
    }
    Object? decoded;
    try {
      decoded = jsonDecode(result.stdout as String);
    } on FormatException {
      throw const FormatException('The sign-in status response was invalid.');
    }
    if (decoded is! Map<String, dynamic> ||
        decoded['ok'] != true ||
        decoded['credentials_present'] is! bool) {
      throw const FormatException('The sign-in status response was invalid.');
    }
    return decoded.cast<String, Object?>();
  }

  Future<ProcessResult> _runProcess(
    String executable,
    List<String> arguments,
  ) => Process.run(executable, arguments, runInShell: false);

  Future<void> _checkStatus() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _statusError = null;
    });
    try {
      final status = await _invokeStatus();
      if (!mounted) return;
      setState(
        () => _credentialsPresent = status['credentials_present'] as bool,
      );
    } on FormatException catch (error) {
      if (!mounted) return;
      setState(() {
        _credentialsPresent = null;
        _statusError = error.message.toString();
      });
    } on ProcessException {
      if (!mounted) return;
      setState(() {
        _credentialsPresent = null;
        _statusError = 'Could not read Antigravity sign-in status.';
      });
    } on FileSystemException {
      if (!mounted) return;
      setState(() {
        _credentialsPresent = null;
        _statusError = 'Could not read Antigravity sign-in status.';
      });
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _openSignIn() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _statusError = null;
    });
    String? helper;
    try {
      helper = await locateWorkspaceHelper(override: widget.helperPath);
      if (helper == null) {
        throw const FormatException(
          'The Antigravity sign-in helper is unavailable.',
        );
      }
      final helperArguments = [helper, 'agy-auth', 'login'];
      await (widget.terminalLauncher ?? _launchTerminal)(
        'x-terminal-emulator',
        ['-e', _pythonExecutable, ...helperArguments],
      );
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text(
            'Complete sign-in in the new terminal, then check status.',
          ),
        ),
      );
    } on FormatException catch (error) {
      if (!mounted) return;
      await _showCommandFallback(error.message.toString());
    } on ProcessException {
      if (!mounted) return;
      await _showCommandFallback(
        'A desktop terminal is unavailable.',
        helper: helper,
      );
    } on FileSystemException {
      if (!mounted) return;
      await _showCommandFallback(
        'The Antigravity sign-in helper is unavailable.',
      );
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _launchTerminal(
    String executable,
    List<String> arguments,
  ) async {
    final process = await Process.start(
      executable,
      arguments,
      runInShell: false,
    );
    process.stdout.listen((_) {});
    process.stderr.listen((_) {});
    final exitCode = await Future.any<int?>([
      process.exitCode.then<int?>((code) => code),
      Future<void>.delayed(
        const Duration(milliseconds: 600),
      ).then<int?>((_) => null),
    ]);
    if (exitCode != null && exitCode != 0) {
      throw ProcessException(
        executable,
        arguments,
        'The terminal launcher exited with status $exitCode.',
        exitCode,
      );
    }
  }

  Future<void> _showCommandFallback(String reason, {String? helper}) async {
    final command = helper == null
        ? null
        : "'$_pythonExecutable' '${helper.replaceAll("'", "'\\''")}' agy-auth login";
    await showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Sign in from a terminal'),
        content: SizedBox(
          width: 480,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(reason),
              const SizedBox(height: 12),
              if (command != null) ...[
                const Text('Copy this command into your desktop terminal:'),
                const SizedBox(height: 8),
                SelectableText(command),
              ] else
                const Text('Update or reinstall OrgBots before signing in.'),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Close'),
          ),
          if (command != null)
            TextButton.icon(
              onPressed: () async {
                await Clipboard.setData(ClipboardData(text: command));
                if (context.mounted) Navigator.pop(context);
              },
              icon: const Icon(Icons.copy),
              label: const Text('Copy command'),
            ),
        ],
      ),
    );
  }

  Widget _actions() => Wrap(
    spacing: 4,
    children: [
      TextButton(
        onPressed: _busy ? null : _checkStatus,
        child: Text(_busy ? 'Checking…' : 'Check sign-in'),
      ),
      FilledButton.tonal(
        onPressed: _busy ? null : _openSignIn,
        child: const Text('Sign in'),
      ),
    ],
  );

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final statusText = switch (_credentialsPresent) {
      true => 'Private sign-in stored · execution verification pending.',
      false => 'No private sign-in is stored yet.',
      null => _statusError ?? 'Sign-in status has not been checked.',
    };
    return Padding(
      padding: const EdgeInsets.only(bottom: 1),
      child: LayoutBuilder(
        builder: (context, constraints) => Material(
          color: Colors.transparent,
          borderRadius: BorderRadius.circular(8),
          child: constraints.maxWidth < 600
              ? Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 16,
                    vertical: 10,
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        'Antigravity CLI',
                        style: Theme.of(context).textTheme.titleMedium,
                      ),
                      const SizedBox(height: 4),
                      Text(
                        statusText,
                        style: TextStyle(
                          fontSize: 12,
                          color: _statusError == null
                              ? scheme.onSurfaceVariant
                              : scheme.error,
                        ),
                      ),
                      const SizedBox(height: 4),
                      const Text(
                        'Uses a separate OrgBots sign-in. Model execution is still disabled.',
                        style: TextStyle(fontSize: 12),
                      ),
                      const SizedBox(height: 4),
                      Align(
                        alignment: Alignment.centerRight,
                        child: _actions(),
                      ),
                    ],
                  ),
                )
              : ListTile(
                  title: const Text('Antigravity CLI'),
                  subtitle: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        statusText,
                        style: TextStyle(
                          fontSize: 12,
                          color: _statusError == null
                              ? scheme.onSurfaceVariant
                              : scheme.error,
                        ),
                      ),
                      const SizedBox(height: 5),
                      const Text(
                        'Uses a separate OrgBots sign-in. Model execution is still disabled.',
                        style: TextStyle(fontSize: 12),
                      ),
                    ],
                  ),
                  trailing: _actions(),
                ),
        ),
      ),
    );
  }
}
