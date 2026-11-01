import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';

import 'workspace_changes_dialog.dart' show WorkspaceProcessRunner;
import 'workspace_helper.dart';

/// Working copies left by bots no longer in the room.
///
/// From the live room: a copy whose baseline was lost was kept, and named in
/// the chat at every room start, with nothing in the app to act on it. This
/// lists every such copy with its size and why it was kept, and discards one
/// the owner picks after the helper saves whatever may be unique in it.
class LeftoverCopiesDialog extends StatefulWidget {
  const LeftoverCopiesDialog({
    super.key,
    required this.stateDirectory,
    required this.keep,
    this.processRunner,
    this.helperPath,
  });

  final Directory stateDirectory;

  /// Copy names that belong to bots in the room; these are never listed.
  final List<String> keep;
  final WorkspaceProcessRunner? processRunner;
  final String? helperPath;

  @override
  State<LeftoverCopiesDialog> createState() => _LeftoverCopiesDialogState();
}

class LeftoverCopy {
  const LeftoverCopy({
    required this.agent,
    required this.bytes,
    required this.reason,
    this.lastModified,
    this.label,
  });

  final String agent;

  /// The name its bot had when it last used the copy, if it was recorded.
  final String? label;
  final int bytes;

  /// When the copy last changed, if known.
  final DateTime? lastModified;

  /// Why the room kept it, or null when nothing in it is unique.
  final String? reason;
}

class _LeftoverCopiesDialogState extends State<LeftoverCopiesDialog> {
  List<LeftoverCopy>? _copies;
  String? _error;
  String? _notice;
  String? _working;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<Map<String, dynamic>> _invoke(List<String> command) async {
    if (!widget.stateDirectory.path.startsWith('/')) {
      throw const FormatException(
        'Room workspace state must be an absolute path.',
      );
    }
    final helper = await locateWorkspaceHelper(override: widget.helperPath);
    if (helper == null) {
      throw const FormatException('The workspace helper is unavailable.');
    }
    final args = <String>[
      helper,
      ...command,
      '--state',
      widget.stateDirectory.path,
      for (final name in widget.keep) ...['--keep', name],
    ];
    final result = await (widget.processRunner ?? Process.run)('python3', args);
    final stdout = result.stdout is String ? result.stdout as String : '';
    Map<String, dynamic>? value;
    try {
      final decoded = jsonDecode(stdout);
      if (decoded is Map) value = decoded.cast<String, dynamic>();
    } on FormatException {
      // Reported below as a process error.
    }
    if (result.exitCode != 0 || value == null || value['ok'] != true) {
      throw FormatException(
        value?['error'] is String
            ? value!['error'] as String
            : 'Workspace helper failed (exit ${result.exitCode}).',
      );
    }
    return value;
  }

  Future<void> _load() async {
    setState(() {
      _copies = null;
      _error = null;
    });
    try {
      final report = await _invoke(['prune', '--dry-run']);
      if (!mounted) return;
      setState(() => _copies = leftoverCopiesFrom(report));
    } catch (error) {
      if (mounted) setState(() => _error = _message(error));
    }
  }

  String _message(Object error) => error is FormatException
      ? error.message.toString()
      : error.toString().replaceFirst('Exception: ', '');

  Future<void> _discard(LeftoverCopy copy) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Discard ${copy.agent}?'),
        content: SizedBox(
          width: 440,
          child: Text(
            copy.reason == null
                ? 'Everything in this copy is already in the project. '
                      'Discarding it frees ${formatCopyBytes(copy.bytes)}.'
                : 'Kept because ${copy.reason}. Before the copy is removed, '
                      'every file that is missing from the project or differs '
                      'from it, and any commits that never landed, are saved '
                      'to a private leftovers folder. Discarding it frees '
                      'about ${formatCopyBytes(copy.bytes)}.',
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Save what differs and discard'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    setState(() {
      _working = copy.agent;
      _error = null;
      _notice = null;
    });
    try {
      final result = await _invoke(['discard', '--agent', copy.agent]);
      if (!mounted) return;
      setState(() => _notice = discardNotice(result));
      await _load();
    } catch (error) {
      if (mounted) setState(() => _error = _message(error));
    } finally {
      if (mounted) setState(() => _working = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    final copies = _copies;
    final scheme = Theme.of(context).colorScheme;
    return AlertDialog(
      title: const Text('Leftover copies'),
      content: SizedBox(
        width: 560,
        height: 420,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'Working copies left by bots that are no longer in this room.',
            ),
            const SizedBox(height: 8),
            if (_notice != null)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: SelectableText(
                  _notice!,
                  key: const ValueKey('leftover-notice'),
                ),
              ),
            if (_error != null)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Text(
                  _error!,
                  key: const ValueKey('leftover-error'),
                  style: TextStyle(color: scheme.error),
                ),
              ),
            if (copies == null && _error == null)
              const LinearProgressIndicator()
            else if (copies != null && copies.isEmpty)
              const Text('No leftover copies.')
            else if (copies != null)
              Expanded(
                child: ListView.separated(
                  itemCount: copies.length,
                  separatorBuilder: (_, _) => const Divider(height: 1),
                  itemBuilder: (context, index) {
                    final copy = copies[index];
                    return ListTile(
                      key: ValueKey('leftover-${copy.agent}'),
                      title: Text(
                        leftoverCopyName(copy.agent, label: copy.label),
                      ),
                      subtitle: Text(
                        [
                          formatCopyBytes(copy.bytes),
                          if (copy.lastModified != null)
                            'last changed ${_day(copy.lastModified!)}',
                          plainLeftoverReason(copy.reason),
                        ].join(' · '),
                      ),
                      trailing: _working == copy.agent
                          ? const SizedBox.square(
                              dimension: 20,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                          : TextButton(
                              onPressed: _working == null
                                  ? () => _discard(copy)
                                  : null,
                              child: const Text('Discard'),
                            ),
                    );
                  },
                ),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: _working == null ? _load : null,
          child: const Text('Refresh'),
        ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Close'),
        ),
      ],
    );
  }
}

/// Every copy a dry-run prune names: the ones it would remove, which hold
/// nothing unique, and the ones it keeps, with the reason.
List<LeftoverCopy> leftoverCopiesFrom(Map<String, dynamic> report) {
  final copies = <LeftoverCopy>[];
  void add(Object? rows, {required bool kept}) {
    if (rows is! List) return;
    for (final row in rows.whereType<Map>()) {
      final agent = row['agent'];
      if (agent is! String) continue;
      final bytes = row['bytes'];
      final modified = row['last_modified'];
      final label = row['label'];
      copies.add(
        LeftoverCopy(
          agent: agent,
          label: label is String && label.isNotEmpty ? label : null,
          bytes: bytes is int ? bytes : 0,
          lastModified: modified is int
              ? DateTime.fromMillisecondsSinceEpoch(modified * 1000)
              : null,
          reason: kept
              ? (row['reason'] is String ? row['reason'] as String : 'kept')
              : null,
        ),
      );
    }
  }

  add(report['kept'], kept: true);
  add(report['removed'], kept: false);
  copies.sort((a, b) => b.bytes.compareTo(a.bytes));
  return copies;
}

String formatCopyBytes(int bytes) {
  if (bytes >= 1e9) return '${(bytes / 1e9).toStringAsFixed(1)} GB';
  if (bytes >= 1e6) return '${(bytes / 1e6).toStringAsFixed(0)} MB';
  if (bytes >= 1e3) return '${(bytes / 1e3).toStringAsFixed(0)} KB';
  return '$bytes bytes';
}

/// What a discard did, in one sentence the owner can act on.
String discardNotice(Map<String, dynamic> result) {
  final agent = result['agent'] is String
      ? result['agent'] as String
      : 'The copy';
  final freed = result['freed_bytes'] is int ? result['freed_bytes'] as int : 0;
  final rescued = result['rescued'] is int ? result['rescued'] as int : 0;
  final history = result['history'] == true;
  final folder = result['rescue_dir'];
  final saved = <String>[
    if (rescued > 0) rescued == 1 ? '1 file' : '$rescued files',
    if (history) 'its unlanded commits',
  ];
  final where = saved.isEmpty || folder is! String
      ? ' Nothing in it differed from the project.'
      : ' Saved ${saved.join(' and ')} to $folder.';
  return 'Discarded $agent, freeing ${formatCopyBytes(freed)}.$where';
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

/// A leftover copy's name as a person reads it. From the live room: the one
/// leftover showed as bot_a1740742eb994413b67d7a3bb1ed7f6e.
///
/// [label] is the name the bot had when it last used the copy, which copies
/// made since 2.2.1 keep with them.
String leftoverCopyName(String agent, {String? label}) {
  final verify = agent.endsWith('-verify');
  final base = verify
      ? agent.substring(0, agent.length - '-verify'.length)
      : agent;
  String name;
  final bot = RegExp(r'^bot_([0-9a-f]{8})[0-9a-f]*$').firstMatch(base);
  if (label != null) {
    name = '$label (no longer in the room)';
  } else if (bot != null) {
    name = 'A bot no longer in the room (${bot.group(1)})';
  } else if (RegExp(r'^([0-9a-f]{2})+$').hasMatch(base)) {
    final bytes = [
      for (var i = 0; i < base.length; i += 2)
        int.parse(base.substring(i, i + 2), radix: 16),
    ];
    name = "@${String.fromCharCodes(bytes)}'s own CLI copy";
  } else {
    name = base;
  }
  return verify ? '$name, verify copy' : name;
}

/// Why a copy was kept, in plain words.
String plainLeftoverReason(String? reason) {
  if (reason == null) return 'nothing in it is unique';
  if (reason.contains('baseline or project could not be read')) {
    return 'OrgBots lost the record of what it started from, so it kept it in case it holds unsaved work';
  }
  final unmerged = RegExp(
    r'^(\d+) change\(s\) never merged',
  ).firstMatch(reason);
  if (unmerged != null) {
    final n = int.parse(unmerged.group(1)!);
    return n == 1
        ? '1 change never reached your project'
        : '$n changes never reached your project';
  }
  if (reason.contains('could not be checked')) {
    return 'OrgBots could not check it for unsaved work';
  }
  if (reason.contains('names another project')) {
    return 'it belongs to another project';
  }
  if (reason.contains('not a private directory')) {
    return 'it is not a folder OrgBots made';
  }
  return reason;
}
