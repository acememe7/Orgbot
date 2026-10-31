import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'workspace_helper.dart';

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
    this.onStatus,
  });

  final Directory stateDirectory;
  final List<WorkspaceAgentLabel> agents;
  final WorkspaceProcessRunner? processRunner;
  final String? helperPath;

  /// Told how many removals each fresh status shows for an agent copy, so
  /// the room's count follows what the owner sees and applies.
  final void Function(String agentId, int deleted)? onStatus;

  @override
  State<WorkspaceChangesDialog> createState() => _WorkspaceChangesDialogState();
}

class _WorkspaceChangesDialogState extends State<WorkspaceChangesDialog> {
  String? _agentId;
  Map<String, dynamic>? _status;
  String? _error;
  bool _busy = false;
  String? _progress;

  @override
  void initState() {
    super.initState();
    _agentId = widget.agents.isEmpty ? null : widget.agents.first.id;
    if (_agentId != null) _refresh();
  }

  Future<String?> _helper() async {
    return locateWorkspaceHelper(override: widget.helperPath);
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
    if (command == 'apply' && widget.processRunner == null) {
      return _decode(await _streamApply(args));
    }
    final result = await (widget.processRunner ?? Process.run)('python3', args);
    return _decode(result);
  }

  /// Run apply while showing its progress lines. From the live room: 1,549
  /// removals ran for thirteen minutes behind a bare progress bar.
  Future<ProcessResult> _streamApply(List<String> args) async {
    final process = await Process.start('python3', args);
    final out = StringBuffer();
    final err = StringBuffer();
    final reading = [
      process.stdout.transform(utf8.decoder).forEach(out.write),
      process.stderr
          .transform(utf8.decoder)
          .transform(const LineSplitter())
          .forEach((line) {
            final progress = workspaceProgressText(line);
            if (progress == null) {
              err.writeln(line);
            } else if (mounted) {
              setState(() => _progress = progress);
            }
          }),
    ];
    final exitCode = await process.exitCode;
    await Future.wait(reading);
    return ProcessResult(process.pid, exitCode, out.toString(), err.toString());
  }

  Map<String, dynamic> _decode(ProcessResult result) {
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
      final agent = _agentId;
      if (agent != null) widget.onStatus?.call(agent, _paths(value['deleted']).length);
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
          // A fixed size and a lazily built list: from the live room, 1,570
          // removals built as one column froze the app on this dialog.
          content: SizedBox(
            width: 520,
            height: 440,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  removals.length == 1
                      ? 'This project file will be moved to your configured recovery folder before the changes are applied:'
                      : 'These ${removals.length} project files will be moved to your configured recovery folder before the changes are applied:',
                ),
                const SizedBox(height: 8),
                PathSummary(paths: removals),
                const SizedBox(height: 8),
                Expanded(
                  child: PathList(paths: removals, keyPrefix: 'approved-delete-'),
                ),
              ],
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
      _progress = null;
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
      final agent = _agentId;
      if (agent != null) widget.onStatus?.call(agent, _paths(value['deleted']).length);
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
            if (_busy && _progress != null)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  _progress!,
                  key: const ValueKey('workspace-apply-progress'),
                ),
              ),
            if (_error != null)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Text(
                  _error!,
                  key: const ValueKey('workspace-status-error'),
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ),
            if (_status != null &&
                changed.isEmpty &&
                added.isEmpty &&
                deleted.isEmpty &&
                conflicts.isEmpty)
              const Padding(
                padding: EdgeInsets.only(top: 8),
                child: Text('No unreviewed workspace changes.'),
              ),
            if (_status != null && conflicts.isNotEmpty)
              const Padding(
                padding: EdgeInsets.only(top: 8),
                child: Text(
                  'Resolve conflicts in the project before applying any changes.',
                ),
              ),
            if (_status != null)
              Expanded(
                child: _ChangeList(
                  sections: [
                    ('Changed', changed),
                    ('Added', added),
                    ('Deleted', deleted),
                    ('Conflicts', conflicts),
                  ],
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

/// How many paths sit in each top-level folder, largest first, so a long
/// list can be judged at a glance before it is read.
class PathSummary extends StatelessWidget {
  const PathSummary({super.key, required this.paths, this.limit = 6});

  final List<String> paths;
  final int limit;

  static List<(String, int)> groups(List<String> paths) {
    final counts = <String, int>{};
    for (final path in paths) {
      final slash = path.indexOf('/');
      final key = slash < 0 ? path : '${path.substring(0, slash)}/';
      counts[key] = (counts[key] ?? 0) + 1;
    }
    final sorted = counts.entries.map((e) => (e.key, e.value)).toList()
      ..sort((a, b) => b.$2 != a.$2 ? b.$2 - a.$2 : a.$1.compareTo(b.$1));
    return sorted;
  }

  @override
  Widget build(BuildContext context) {
    final grouped = groups(paths);
    if (grouped.length < 2 && paths.length < 20) return const SizedBox.shrink();
    final shown = grouped.take(limit).toList();
    final rest = grouped.skip(limit).fold<int>(0, (sum, group) => sum + group.$2);
    final style = Theme.of(context).textTheme.bodySmall;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final (folder, count) in shown)
          Text(
            folder.endsWith('/')
                ? '$count ${count == 1 ? 'file' : 'files'} in $folder'
                : folder,
            style: style,
          ),
        if (rest > 0) Text('$rest more elsewhere', style: style),
      ],
    );
  }
}

/// Every path, built only as it scrolls into view.
class PathList extends StatelessWidget {
  const PathList({super.key, required this.paths, required this.keyPrefix});

  final List<String> paths;
  final String keyPrefix;

  @override
  Widget build(BuildContext context) {
    return Scrollbar(
      child: ListView.builder(
        itemCount: paths.length,
        itemExtent: 24,
        itemBuilder: (context, index) => Text(
          paths[index],
          key: ValueKey('$keyPrefix${paths[index]}'),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
      ),
    );
  }
}

/// The review's sections as one lazily built list: a heading and summary per
/// section, then its paths.
class _ChangeList extends StatelessWidget {
  const _ChangeList({required this.sections});

  final List<(String, List<String>)> sections;

  @override
  Widget build(BuildContext context) {
    final rows = <Widget Function(BuildContext)>[];
    for (final (title, paths) in sections) {
      if (paths.isEmpty) continue;
      rows.add(
        (context) => Padding(
          padding: const EdgeInsets.only(top: 12, bottom: 4),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                '$title (${paths.length})',
                style: Theme.of(context).textTheme.titleSmall,
              ),
              PathSummary(paths: paths),
            ],
          ),
        ),
      );
      for (final path in paths) {
        rows.add(
          (context) => Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(
              path,
              key: ValueKey('workspace-path-$title-$path'),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
        );
      }
    }
    return Scrollbar(
      child: ListView.builder(
        itemCount: rows.length,
        itemBuilder: (context, index) => rows[index](context),
      ),
    );
  }
}

/// The words for one of the workspace helper's progress lines, or null when
/// the line is something else (such as an error to show).
String? workspaceProgressText(String line) {
  if (!line.startsWith('{"progress"')) return null;
  try {
    final decoded = jsonDecode(line);
    final progress = decoded is Map ? decoded['progress'] : null;
    if (progress is! Map) return null;
    final done = progress['done'];
    final total = progress['total'];
    if (done is! int || total is! int) return null;
    return switch (progress['stage']) {
      'trash' => 'Moved $done of $total to the trash',
      'apply' => 'Applied $done of $total changes',
      _ => null,
    };
  } on FormatException {
    return null;
  }
}
