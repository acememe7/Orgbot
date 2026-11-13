import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'leftover_copies_dialog.dart';
import 'workspace_helper.dart';

typedef WorkspaceProcessRunner =
    Future<ProcessResult> Function(String executable, List<String> arguments);

class WorkspaceAgentLabel {
  const WorkspaceAgentLabel({
    required this.id,
    required this.label,
    this.removals = 0,
  });

  final String id;
  final String label;

  /// File removals the room last counted in this copy.
  final int removals;
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
    this.leftoverKeep,
  });

  final Directory stateDirectory;
  final List<WorkspaceAgentLabel> agents;
  final WorkspaceProcessRunner? processRunner;
  final String? helperPath;

  /// Told how many removals each fresh status shows for an agent copy, so
  /// the room's count follows what the owner sees and applies.
  final void Function(String agentId, int deleted)? onStatus;

  /// Copy names of the bots in the room. When given, the dialog offers the
  /// leftover copies of every other bot.
  final List<String>? leftoverKeep;

  @override
  State<WorkspaceChangesDialog> createState() => _WorkspaceChangesDialogState();
}

class _WorkspaceChangesDialogState extends State<WorkspaceChangesDialog> {
  String? _agentId;

  /// Removals per copy, as last counted, shown beside each copy's name.
  ///
  /// From the live room: the badge said 2,177 and the owner could not tell
  /// which bot they belonged to; 1,588 of them were in a copy that no longer
  /// had any, because a count only changed when that bot ran.
  late final Map<String, int> _counts = {
    for (final agent in widget.agents) agent.id: agent.removals,
  };
  Map<String, dynamic>? _status;
  String? _error;
  bool _busy = false;
  String? _progress;

  @override
  void initState() {
    super.initState();
    // Open on the copy with the most removals waiting.
    final sorted = [...widget.agents]
      ..sort((a, b) => b.removals.compareTo(a.removals));
    _agentId = sorted.isEmpty ? null : sorted.first.id;
    if (_agentId != null) _refresh().then((_) => _recount());
  }

  Future<String?> _helper() async {
    return locateWorkspaceHelper(override: widget.helperPath);
  }

  Future<Map<String, dynamic>> _invoke(
    String command, {
    String? reviewToken,
    String? agentId,
    List<String> onlyFolders = const [],
  }) async {
    final agent = agentId ?? _agentId;
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
    for (final folder in onlyFolders) {
      args.addAll(['--only-folder', folder]);
    }
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
      final agent = _agentId;
      if (mounted) {
        setState(() {
          _status = value;
          if (agent != null) _counts[agent] = _paths(value['deleted']).length;
        });
      }
      if (agent != null) {
        widget.onStatus?.call(agent, _paths(value['deleted']).length);
      }
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

  /// Check again every other copy the room counts removals in, one at a
  /// time, so a count left over from before is corrected. Read-only.
  Future<void> _recount() async {
    final stale = [
      for (final agent in widget.agents)
        if (agent.id != _agentId && (_counts[agent.id] ?? 0) > 0) agent.id,
    ];
    for (final id in stale) {
      if (!mounted) return;
      try {
        final value = await _invoke('status', agentId: id);
        final deleted = _paths(value['deleted']).length;
        if (!mounted) return;
        // The copy opened meanwhile keeps its own, newer count. From the 13
        // November audit: a late answer here restored a count just cleared.
        if (id == _agentId) continue;
        setState(() => _counts[id] = deleted);
        widget.onStatus?.call(id, deleted);
      } catch (_) {
        // Left as it was; opening that copy shows why.
      }
    }
  }

  void _select(String? agentId) {
    if (agentId == null || agentId == _agentId) return;
    setState(() {
      _agentId = agentId;
      _status = null;
    });
    _refresh();
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
      final groups = displayed['deleted_groups'] is List
          ? (displayed['deleted_groups'] as List).whereType<Map>().toList()
          : null;
      final folders = await showDialog<List<String>>(
        context: context,
        builder: (context) =>
            ApproveRemovalsDialog(removals: removals, groups: groups),
      );
      if (folders == null || !mounted) return;
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
      await _applyCommand(reviewToken: token, onlyFolders: folders);
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

  Future<void> _applyCommand({
    String? reviewToken,
    List<String> onlyFolders = const [],
  }) async {
    setState(() {
      _busy = true;
      _error = null;
      _progress = null;
    });
    try {
      await _invoke(
        'apply',
        reviewToken: reviewToken,
        onlyFolders: onlyFolders,
      );
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
    if (agent != null) {
      widget.onStatus?.call(agent, _paths(value['deleted']).length);
    }
  }

  @override
  Widget build(BuildContext context) {
    final selected = widget.agents.any((agent) => agent.id == _agentId);
    final changed = _paths(_status?['changed'], objectPaths: true);
    final added = _paths(_status?['added']);
    final deleted = _paths(_status?['deleted']);
    final conflicts = _paths(_status?['conflicts']);
    // Not closable while a merge or refresh runs. From the 13 November audit:
    // closing during an apply left it running with no report of how it went.
    return PopScope(
      canPop: !_busy,
      child: _dialog(context, changed, added, deleted, conflicts, selected),
    );
  }

  Widget _dialog(
    BuildContext context,
    List<String> changed,
    List<String> added,
    List<String> deleted,
    List<String> conflicts,
    bool selected,
  ) {
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
                // Keyed by the selection: the overview below can change it.
                key: ValueKey('copy-picker-$_agentId'),
                isExpanded: true,
                initialValue: _agentId,
                decoration: const InputDecoration(labelText: 'Agent'),
                items: [
                  for (final agent in widget.agents)
                    DropdownMenuItem(
                      value: agent.id,
                      child: Text(
                        workspaceCopyLabel(agent.label, _counts[agent.id] ?? 0),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                ],
                onChanged: _busy ? null : _select,
              ),
            if (_counts.values.where((n) => n > 0).length > 1)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: CopyOverview(
                  agents: widget.agents,
                  counts: _counts,
                  selected: _agentId,
                  onSelect: _busy ? null : _select,
                ),
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
            if (_status != null &&
                deleted.isNotEmpty &&
                _status!['deleted_groups'] is List)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: RemovalGroups(
                  groups: (_status!['deleted_groups'] as List)
                      .whereType<Map>()
                      .toList(),
                ),
              ),
            if (_status != null &&
                !_busy &&
                (conflicts.isNotEmpty ||
                    (changed.isEmpty && added.isEmpty && deleted.isEmpty)))
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  conflicts.isNotEmpty
                      ? 'Apply is off until the conflicts are resolved.'
                      : 'Apply is off: there is nothing to apply.',
                  key: const ValueKey('apply-off-reason'),
                  style: TextStyle(
                    fontSize: 12,
                    color: Theme.of(context).colorScheme.onSurfaceVariant,
                  ),
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
        if (widget.leftoverKeep != null)
          TextButton(
            onPressed: _busy
                ? null
                : () => showDialog<void>(
                    context: context,
                    builder: (context) => LeftoverCopiesDialog(
                      stateDirectory: widget.stateDirectory,
                      keep: widget.leftoverKeep!,
                      processRunner: widget.processRunner,
                      helperPath: widget.helperPath,
                    ),
                  ),
            child: const Text('Leftover copies'),
          ),
        TextButton(
          onPressed: _busy ? null : _refresh,
          child: const Text('Refresh'),
        ),
        TextButton(
          onPressed: _busy ? null : () => Navigator.pop(context),
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
    final rest = grouped
        .skip(limit)
        .fold<int>(0, (sum, group) => sum + group.$2);
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

/// A copy's name in the review list, with its waiting removals.
String workspaceCopyLabel(String label, int removals) => removals <= 0
    ? label
    : '$label · $removals ${removals == 1 ? 'removal' : 'removals'}';

/// What a removal review would take, by folder: files, size, when the newest
/// was last changed and whether git tracks them. Recent changes are marked:
/// from the live room, a folder among 562 removals had been written to
/// minutes before, and only another session noticed.
class RemovalGroups extends StatelessWidget {
  const RemovalGroups({
    super.key,
    required this.groups,
    this.now,
    this.chosen,
    this.onToggle,
  });

  final List<Map> groups;

  /// The time to measure "changed … ago" from; the clock if not given.
  final DateTime? now;

  /// The folders ticked, when the owner is choosing which to approve. Every
  /// folder is listed then, each with a checkbox.
  final Set<String>? chosen;
  final ValueChanged<String>? onToggle;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final clock = now ?? DateTime.now();
    return Column(
      key: const ValueKey('removal-groups'),
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final group in chosen == null ? groups.take(8) : groups)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 3),
            child: Builder(
              builder: (context) {
                final files = group['files'] is int ? group['files'] as int : 0;
                final bytes = group['bytes'] is int ? group['bytes'] as int : 0;
                final tracked = group['tracked'] is int
                    ? group['tracked'] as int
                    : 0;
                final newest = group['newest'] is int
                    ? DateTime.fromMillisecondsSinceEpoch(
                        (group['newest'] as int) * 1000,
                      )
                    : null;
                final age = newest == null ? null : clock.difference(newest);
                final recent = age != null && age < const Duration(hours: 1);
                final git = tracked == 0
                    ? 'not in git'
                    : tracked == files
                    ? 'in git'
                    : '$tracked of $files in git';
                final origin = removalOrigin(group, clock);
                final folder = '${group['folder']}';
                return Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    if (chosen != null)
                      Checkbox(
                        key: ValueKey('choose-folder-$folder'),
                        value: chosen!.contains(folder),
                        visualDensity: VisualDensity.compact,
                        onChanged: (_) => onToggle?.call(folder),
                      ),
                    Expanded(
                      child: Text.rich(
                        TextSpan(
                          children: [
                            TextSpan(
                              text: '${group['folder']}  ',
                              style: const TextStyle(
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                            TextSpan(
                              text:
                                  '$files ${files == 1 ? 'file' : 'files'} · ${_size(bytes)} · $git',
                              style: TextStyle(color: scheme.onSurfaceVariant),
                            ),
                            if (origin != null)
                              TextSpan(
                                text: '\n$origin',
                                style: TextStyle(
                                  fontSize: 12,
                                  color: scheme.onSurfaceVariant,
                                ),
                              ),
                          ],
                        ),
                        style: const TextStyle(fontSize: 12.5),
                      ),
                    ),
                    if (age != null)
                      Text(
                        recent
                            ? 'changed ${_ago(age)}, may be in use'
                            : 'changed ${_ago(age)}',
                        style: TextStyle(
                          fontSize: 12,
                          color: recent
                              ? Colors.amber.shade400
                              : scheme.onSurfaceVariant,
                        ),
                      ),
                  ],
                );
              },
            ),
          ),
        if (chosen == null && groups.length > 8)
          Text(
            'and ${groups.length - 8} more',
            style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
          ),
      ],
    );
  }

  static String _size(int bytes) {
    if (bytes >= 1e9) return '${(bytes / 1e9).toStringAsFixed(1)} GB';
    if (bytes >= 1e6) return '${(bytes / 1e6).toStringAsFixed(0)} MB';
    if (bytes >= 1e3) return '${(bytes / 1e3).toStringAsFixed(0)} KB';
    return '$bytes bytes';
  }

  static String _ago(Duration age) {
    if (age.inMinutes < 1) return 'just now';
    if (age.inHours < 1) return '${age.inMinutes} min ago';
    if (age.inDays < 1) return '${age.inHours} h ago';
    return '${age.inDays} d ago';
  }
}

/// Which bots added a removal group's files and which last changed one, as
/// far as OrgBots applied their work, and for older tracked files the commit
/// that brought them in, or null when nothing is known.
String? removalOrigin(Map group, DateTime now) {
  final files = group['files'] is int ? group['files'] as int : 0;
  final parts = <String>[];
  final makers = [
    for (final row in group['made_by'] is List ? group['made_by'] as List : [])
      if (row is List && row.length == 2 && row[0] is String && row[1] is int)
        (row[0] as String, row[1] as int),
  ];
  final known = makers.fold<int>(0, (sum, m) => sum + m.$2);
  if (makers.isNotEmpty) {
    parts.add(
      makers.length == 1 && known == files
          ? 'added by ${makers.single.$1}'
          : 'added by ${makers.map((m) => '${m.$1} (${m.$2})').join(', ')}',
    );
  }
  final arrived = group['git_added'];
  final gitFiles = group['git_files'] is int ? group['git_files'] as int : 0;
  if (arrived is Map &&
      arrived['commit'] is String &&
      arrived['at'] is int &&
      gitFiles > 0) {
    final commits = arrived['commits'] is int ? arrived['commits'] as int : 1;
    final subject = arrived['subject'] is String
        ? ' "${arrived['subject']}"'
        : '';
    final when = _day(
      DateTime.fromMillisecondsSinceEpoch((arrived['at'] as int) * 1000),
    );
    final which = gitFiles == files ? '' : '$gitFiles ';
    parts.add(
      commits > 1
          ? '${which}came in over $commits commits, latest ${arrived['commit']}$subject on $when'
          : '${which}came in with ${arrived['commit']}$subject on $when',
    );
  }
  final rest = files - known - gitFiles;
  if (rest > 0 && parts.isNotEmpty) parts.add('$rest with no record');
  final lastBy = group['last_by'];
  final lastAt = group['last_at'];
  if (lastBy is String && lastAt is int) {
    final age = now.difference(
      DateTime.fromMillisecondsSinceEpoch(lastAt * 1000),
    );
    parts.add('last changed by $lastBy ${RemovalGroups._ago(age)}');
  }
  return parts.isEmpty ? null : parts.join(' · ');
}

String _day(DateTime when) {
  const months = [
    'Jan',
    'Feb',
    'Mar',
    'Apr',
    'May',
    'Jun',
    'Jul',
    'Aug',
    'Sep',
    'Oct',
    'Nov',
    'Dec',
  ];
  final local = when.toLocal();
  return '${local.day} ${months[local.month - 1]}';
}

/// Approve a removal review whole, or only some of its folders.
///
/// Pops with the folders to approve (empty for all of them), or null when
/// cancelled. From the live room: one approval had to take all 562 removals
/// or none, while one folder among them had just been written to.
class ApproveRemovalsDialog extends StatefulWidget {
  const ApproveRemovalsDialog({
    super.key,
    required this.removals,
    required this.groups,
  });

  final List<String> removals;

  /// The review's folders, or null from a helper too old to group them,
  /// which cannot approve by folder either.
  final List<Map>? groups;

  @override
  State<ApproveRemovalsDialog> createState() => _ApproveRemovalsDialogState();
}

class _ApproveRemovalsDialogState extends State<ApproveRemovalsDialog> {
  /// The folders ticked, while choosing; null approves everything.
  Set<String>? _chosen;

  int _filesIn(Set<String> folders) => [
    for (final group in widget.groups ?? const <Map>[])
      if (folders.contains('${group['folder']}') && group['files'] is int)
        group['files'] as int,
  ].fold(0, (sum, n) => sum + n);

  @override
  Widget build(BuildContext context) {
    final removals = widget.removals;
    final groups = widget.groups;
    final chosen = _chosen;
    final all = groups == null
        ? const <String>[]
        : [for (final group in groups) '${group['folder']}'];
    final every = chosen == null || chosen.length == all.length;
    final count = every ? removals.length : _filesIn(chosen);
    return AlertDialog(
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
              chosen != null
                  ? 'Tick the folders to move to your recovery folder. The rest keep waiting for review.'
                  : removals.length == 1
                  ? 'This project file will be moved to your configured recovery folder before the changes are applied:'
                  : 'These ${removals.length} project files will be moved to your configured recovery folder before the changes are applied:',
            ),
            const SizedBox(height: 8),
            if (groups != null && chosen != null)
              Expanded(
                child: SingleChildScrollView(
                  child: RemovalGroups(
                    groups: groups,
                    chosen: chosen,
                    onToggle: (folder) => setState(() {
                      if (!chosen.remove(folder)) chosen.add(folder);
                    }),
                  ),
                ),
              )
            else ...[
              if (groups != null)
                RemovalGroups(groups: groups)
              else
                PathSummary(paths: removals),
              const SizedBox(height: 8),
              Expanded(
                child: PathList(paths: removals, keyPrefix: 'approved-delete-'),
              ),
            ],
          ],
        ),
      ),
      actions: [
        if (groups != null && groups.length > 1 && chosen == null)
          TextButton(
            key: const ValueKey('choose-folders'),
            onPressed: () => setState(() => _chosen = {...all}),
            child: const Text('Choose folders'),
          ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: count == 0
              ? null
              : () => Navigator.pop(context, every ? <String>[] : [...chosen]),
          child: Text(
            every
                ? 'Move listed files to Trash and apply'
                : 'Move $count ${count == 1 ? 'file' : 'files'} to Trash and apply',
          ),
        ),
      ],
    );
  }
}

/// Every copy with removals waiting, with its count, one tap from its review.
///
/// From the live room: 2,177 removals across copies, and the owner had to
/// open the picker to learn there were two bots behind them.
class CopyOverview extends StatelessWidget {
  const CopyOverview({
    super.key,
    required this.agents,
    required this.counts,
    required this.selected,
    required this.onSelect,
  });

  final List<WorkspaceAgentLabel> agents;
  final Map<String, int> counts;
  final String? selected;
  final ValueChanged<String>? onSelect;

  @override
  Widget build(BuildContext context) {
    final waiting = [
      for (final agent in agents)
        if ((counts[agent.id] ?? 0) > 0) agent,
    ]..sort((a, b) => (counts[b.id] ?? 0).compareTo(counts[a.id] ?? 0));
    final total = waiting.fold<int>(0, (sum, a) => sum + (counts[a.id] ?? 0));
    return Column(
      key: const ValueKey('copy-overview'),
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          '$total ${total == 1 ? 'removal' : 'removals'} waiting in ${waiting.length} copies',
          style: TextStyle(
            fontSize: 12,
            color: Theme.of(context).colorScheme.onSurfaceVariant,
          ),
        ),
        const SizedBox(height: 4),
        Wrap(
          spacing: 6,
          runSpacing: 4,
          children: [
            for (final agent in waiting)
              ChoiceChip(
                key: ValueKey('copy-chip-${agent.id}'),
                label: Text(workspaceCopyLabel(agent.label, counts[agent.id]!)),
                selected: agent.id == selected,
                visualDensity: VisualDensity.compact,
                onSelected: onSelect == null
                    ? null
                    : (_) => onSelect!(agent.id),
              ),
          ],
        ),
      ],
    );
  }
}
