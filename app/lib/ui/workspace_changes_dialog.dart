import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';

typedef WorkspaceProcessRunner =
    Future<ProcessResult> Function(String executable, List<String> arguments);

class WorkspaceAgentLabel {
  const WorkspaceAgentLabel({required this.id, required this.label});

  final String id;
  final String label;
}

/// Owner review of isolated agent changes. Status is read-only; apply is a
/// separate action and deletions require an explicit, path-specific approval.
class WorkspaceChangesDialog extends StatefulWidget {
  const WorkspaceChangesDialog({
    super.key,
    required this.stateDirectory,
    required this.agents,
    this.processRunner,
    this.helperPath,
  });

  final Directory stateDirectory;
  final List<WorkspaceAgentLabel> agents;
  final WorkspaceProcessRunner? processRunner;
  final String? helperPath;

  @override
  State<WorkspaceChangesDialog> createState() => _WorkspaceChangesDialogState();
}

class _WorkspaceChangesDialogState extends State<WorkspaceChangesDialog> {
  String? _agentId;
  Map<String, dynamic>? _status;
  String? _error;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _agentId = widget.agents.isEmpty ? null : widget.agents.first.id;
    if (_agentId != null) _refresh();
  }

  Future<String?> _helper() async {
    if (widget.helperPath != null) return widget.helperPath;
    const installed = '/usr/lib/orgbot/orgbot-workspace.py';
    if (await File(installed).exists()) return installed;
    final candidates = [
      '${Directory.current.path}/app/packaging/orgbot-workspace.py',
      '${Directory.current.path}/packaging/orgbot-workspace.py',
      '${File(Platform.resolvedExecutable).parent.path}/orgbot-workspace.py',
    ];
    for (final candidate in candidates) {
      if (await File(candidate).exists()) return candidate;
    }
    return null;
  }

  Future<Map<String, dynamic>> _invoke(
    String command, {
    String? reviewToken,
  }) async {
    final agent = _agentId;
    if (agent == null ||
        !RegExp(r'^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$').hasMatch(agent)) {
      throw const FormatException(
        'The selected room agent has an invalid identity.',
      );
    }
    if (!widget.stateDirectory.path.startsWith('/')) {
      throw const FormatException(
        'Room workspace state must be an absolute path.',
      );
    }
    final helper = await _helper();
    if (helper == null) {
      throw const FormatException('The workspace helper is unavailable.');
    }
    final args = <String>[
      helper,
      command,
      '--state',
      widget.stateDirectory.path,
      '--agent',
      agent,
    ];
    if (reviewToken != null) args.addAll(['--review-token', reviewToken]);
    final result = await (widget.processRunner ?? Process.run)('python3', args);
    final stdout = result.stdout is String ? result.stdout as String : '';
    Map<String, dynamic>? value;
    try {
      final decoded = jsonDecode(stdout);
      if (decoded is Map) value = decoded.cast<String, dynamic>();
    } on FormatException {
      // A non-JSON helper failure is rendered below as a concise process error.
    }
    if (result.exitCode != 0 || value == null || value['ok'] != true) {
      final message = value?['error'] is String
          ? value!['error'] as String
          : (result.stderr is String &&
                    (result.stderr as String).trim().isNotEmpty
                ? (result.stderr as String).trim()
                : 'Workspace helper failed (exit ${result.exitCode}).');
      throw FormatException(message);
    }
    return value;
  }

  Future<void> _refresh() async {
    if (_busy || _agentId == null) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final value = await _invoke('status');
      if (mounted) setState(() => _status = value);
    } catch (error) {
      if (mounted) {
        setState(() {
          _status = null;
          _error = _message(error);
        });
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  String _message(Object error) => error is FormatException
      ? error.message.toString()
      : error.toString().replaceFirst('Exception: ', '');

  List<String> _paths(Object? value, {bool objectPaths = false}) {
    if (value is! List) return const [];
    return value
        .map((item) {
          if (item is String) return item;
          if (objectPaths && item is Map && item['path'] is String) {
            return item['path'] as String;
          }
          return null;
        })
        .whereType<String>()
        .toList(growable: false);
  }

  Future<void> _apply() async {
    final displayed = _status;
    if (displayed == null || _paths(displayed['conflicts']).isNotEmpty) return;
    final removals = _paths(displayed['deleted']);
    if (removals.isNotEmpty) {
      final approved = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('Approve deleted files'),
          content: SizedBox(
            width: 520,
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Text(
                    'These exact project files will be moved to HDD Trash before the changes are applied:',
                  ),
                  const SizedBox(height: 12),
                  for (final path in removals)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 6),
                      child: SelectableText(
                        path,
                        key: ValueKey('approved-delete-$path'),
                      ),
                    ),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Move listed files to Trash and apply'),
            ),
          ],
        ),
      );
      if (approved != true || !mounted) return;
      // Re-read status after approval. If the deletion set changed, the
      // approval no longer covers the operation and nothing is applied.
      await _refresh();
      if (!mounted || _status == null) return;
      if (!_sameReview(displayed, _status!)) {
        setState(
          () => _error =
              'Workspace changes changed during review. Review the refreshed list before applying.',
        );
        return;
      }
      if (_paths(_status!['conflicts']).isNotEmpty) return;
      final token = displayed['review_token'];
      if (token is! String || !RegExp(r'^[a-f0-9]{64}$').hasMatch(token)) {
        setState(
          () => _error =
              'This workspace helper cannot verify the approval. Update OrgBots before applying removals.',
        );
        return;
      }
      await _applyCommand(reviewToken: token);
    } else {
      await _applyCommand();
    }
  }

  bool _samePaths(List<String> a, List<String> b) {
    final left = [...a]..sort();
    final right = [...b]..sort();
    if (left.length != right.length) return false;
    for (var i = 0; i < left.length; i++) {
      if (left[i] != right[i]) return false;
    }
    return true;
  }

  bool _sameReview(Map<String, dynamic> before, Map<String, dynamic> after) {
    if (before['review_token'] != after['review_token']) return false;
    if (!_samePaths(_paths(before['added']), _paths(after['added'])) ||
        !_samePaths(_paths(before['deleted']), _paths(after['deleted'])) ||
        !_samePaths(_paths(before['conflicts']), _paths(after['conflicts']))) {
      return false;
    }
    final oldChanged = _paths(before['changed'], objectPaths: true)..sort();
    final newChanged = _paths(after['changed'], objectPaths: true)..sort();
    if (!_samePaths(oldChanged, newChanged)) return false;
    final oldKinds = _changeKinds(before['changed']);
    final newKinds = _changeKinds(after['changed']);
    return _samePaths(oldKinds, newKinds);
  }

  List<String> _changeKinds(Object? value) {
    if (value is! List) return const [];
    return value
        .whereType<Map>()
        .map((row) => '${row['path']}:${row['kind']}')
        .toList()
      ..sort();
  }

  Future<void> _applyCommand({String? reviewToken}) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await _invoke('apply', reviewToken: reviewToken);
      await _refreshAfterApply();
    } catch (error) {
      if (mounted) setState(() => _error = _message(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _refreshAfterApply() async {
    final value = await _invoke('status');
    if (mounted) setState(() => _status = value);
  }

  @override
  Widget build(BuildContext context) {
    final selected = widget.agents.any((agent) => agent.id == _agentId);
    final changed = _paths(_status?['changed'], objectPaths: true);
    final added = _paths(_status?['added']);
    final deleted = _paths(_status?['deleted']);
    final conflicts = _paths(_status?['conflicts']);
    return AlertDialog(
      title: const Text('Agent workspace changes'),
      content: SizedBox(
        width: 560,
        height: 480,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (widget.agents.isEmpty)
              const Text('No live room agents are available to inspect.')
            else
              DropdownButtonFormField<String>(
                initialValue: _agentId,
                decoration: const InputDecoration(labelText: 'Agent'),
                items: [
                  for (final agent in widget.agents)
                    DropdownMenuItem(value: agent.id, child: Text(agent.label)),
                ],
                onChanged: _busy
                    ? null
                    : (value) {
                        setState(() {
                          _agentId = value;
                          _status = null;
                        });
                        _refresh();
                      },
              ),
            const SizedBox(height: 12),
            if (_busy) const LinearProgressIndicator(),
            if (_error != null)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Text(
                  _error!,
                  key: const ValueKey('workspace-status-error'),
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ),
            if (_status != null)
              Expanded(
                child: SingleChildScrollView(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      if (changed.isEmpty &&
                          added.isEmpty &&
                          deleted.isEmpty &&
                          conflicts.isEmpty)
                        const Padding(
                          padding: EdgeInsets.only(top: 8),
                          child: Text('No unreviewed workspace changes.'),
                        ),
                      for (final entry in [
                        ('Changed', changed),
                        ('Added', added),
                        ('Deleted', deleted),
                        ('Conflicts', conflicts),
                      ])
                        if (entry.$2.isNotEmpty) ...[
                          Padding(
                            padding: const EdgeInsets.only(top: 12),
                            child: Text(
                              '${entry.$1} (${entry.$2.length})',
                              style: Theme.of(context).textTheme.titleSmall,
                            ),
                          ),
                          for (final path in entry.$2)
                            Padding(
                              padding: const EdgeInsets.only(top: 4),
                              child: SelectableText(
                                path,
                                key: ValueKey(
                                  'workspace-path-${entry.$1}-$path',
                                ),
                              ),
                            ),
                        ],
                      if (conflicts.isNotEmpty)
                        const Padding(
                          padding: EdgeInsets.only(top: 8),
                          child: Text(
                            'Resolve conflicts in the project before applying any changes.',
                          ),
                        ),
                    ],
                  ),
                ),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: _busy ? null : _refresh,
          child: const Text('Refresh'),
        ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Close'),
        ),
        FilledButton.icon(
          onPressed:
              _busy ||
                  _status == null ||
                  changed.isEmpty && added.isEmpty && deleted.isEmpty ||
                  conflicts.isNotEmpty ||
                  !selected
              ? null
              : _apply,
          icon: const Icon(Icons.merge_type),
          label: const Text('Apply changes'),
        ),
      ],
    );
  }
}
