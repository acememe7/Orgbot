import 'package:flutter/material.dart';

import '../orgbot/lend_settings.dart';

/// Your sign-ins bots may ask to use, one approved task at a time.
///
/// Off by default: a bot sees none of these unless you turn one on here and
/// its request for a task is approved, by you or by the room's access mode.
class LendSettingsView extends StatefulWidget {
  const LendSettingsView({super.key, this.load = LendSettings.load});

  /// Where the list comes from; replaceable in tests.
  final LendSettings Function() load;

  @override
  State<LendSettingsView> createState() => _LendSettingsViewState();
}

class _LendSettingsViewState extends State<LendSettingsView> {
  LendSettings? _settings;
  String? _error;

  void _open() {
    try {
      setState(() {
        _settings = widget.load();
        _error = null;
      });
    } on Object catch (error) {
      setState(() => _error = 'Could not read your sign-in list: $error');
    }
  }

  void _change(VoidCallback update) {
    final settings = _settings;
    if (settings == null) return;
    setState(update);
    try {
      settings.save();
      _error = null;
    } on Object catch (error) {
      setState(() => _error = 'Could not save your sign-in list: $error');
    }
  }

  Future<void> _add() async {
    final settings = _settings;
    if (settings == null) return;
    final added = await showDialog<LendEntry>(
      context: context,
      builder: (context) => _AddSignInDialog(taken: {for (final e in settings.entries) e.name}),
    );
    if (added != null) _change(() => settings.entries.add(added));
  }

  @override
  Widget build(BuildContext context) {
    final settings = _settings;
    return ExpansionTile(
      title: const Text('Sign-ins bots may use'),
      leading: const Icon(Icons.key_outlined),
      onExpansionChanged: (open) {
        if (open) _open();
      },
      children: [
        const ListTile(
          dense: true,
          title: Text(
            'Bots never see these unless you turn one on and a request for a task is '
            'approved. The room\'s Access setting decides which requests ask you. '
            'Changes apply when a room next starts.',
          ),
        ),
        if (_error != null) ListTile(title: Text(_error!)),
        if (settings != null)
          for (final entry in settings.entries)
            _LendTile(
              key: ValueKey('lend-${entry.name}'),
              entry: entry,
              present: settings.isPresent(entry),
              onEnabled: (value) => _change(() => entry.enabled = value),
              onRisk: (risk) => _change(() => entry.risk = risk),
              onRemove: entry.builtIn
                  ? null
                  : () => _change(() => settings.entries.remove(entry)),
            ),
        Align(
          alignment: Alignment.centerRight,
          child: TextButton.icon(
            onPressed: settings == null ? null : _add,
            icon: const Icon(Icons.add),
            label: const Text('Add a sign-in'),
          ),
        ),
      ],
    );
  }
}

class _LendTile extends StatelessWidget {
  const _LendTile({
    super.key,
    required this.entry,
    required this.present,
    required this.onEnabled,
    required this.onRisk,
    this.onRemove,
  });

  final LendEntry entry;
  final bool present;
  final ValueChanged<bool> onEnabled;
  final ValueChanged<LendRisk> onRisk;
  final VoidCallback? onRemove;

  @override
  Widget build(BuildContext context) => ListTile(
    title: Text(entry.label),
    subtitle: Text(
      '${entry.paths.map((path) => '~/$path').join(', ')} · '
      '${present ? 'on this computer' : 'not found here'} · bots ask for "${entry.name}"',
      maxLines: 2,
      overflow: TextOverflow.ellipsis,
    ),
    trailing: Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        DropdownButton<LendRisk>(
          value: entry.risk,
          underline: const SizedBox.shrink(),
          items: [
            for (final risk in LendRisk.values)
              DropdownMenuItem(value: risk, child: Text(risk.label)),
          ],
          onChanged: (risk) {
            if (risk != null) onRisk(risk);
          },
        ),
        if (onRemove != null)
          IconButton(
            tooltip: 'Remove ${entry.label}',
            onPressed: onRemove,
            icon: const Icon(Icons.delete_outline),
          ),
        Switch(
          value: entry.enabled,
          onChanged: onEnabled,
        ),
      ],
    ),
  );
}

class _AddSignInDialog extends StatefulWidget {
  const _AddSignInDialog({required this.taken});

  final Set<String> taken;

  @override
  State<_AddSignInDialog> createState() => _AddSignInDialogState();
}

class _AddSignInDialogState extends State<_AddSignInDialog> {
  final _name = TextEditingController();
  final _label = TextEditingController();
  final _path = TextEditingController();
  var _risk = LendRisk.risky;
  String? _problem;

  @override
  void dispose() {
    _name.dispose();
    _label.dispose();
    _path.dispose();
    super.dispose();
  }

  void _submit() {
    final name = _name.text.trim();
    final path = _path.text.trim().replaceFirst(RegExp(r'^~/'), '');
    String? problem;
    if (!lendNamePattern.hasMatch(name) || name == 'server') {
      problem = 'Name: lowercase letters, digits, - or _ (not "server")';
    } else if (widget.taken.contains(name)) {
      problem = 'That name is already in the list';
    } else {
      problem = lendPathProblem(path);
    }
    if (problem != null) {
      setState(() => _problem = problem);
      return;
    }
    Navigator.pop(
      context,
      LendEntry(
        name: name,
        label: _label.text.trim().isEmpty ? name : _label.text.trim(),
        paths: [path],
        risk: _risk,
        enabled: true,
      ),
    );
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Add a sign-in'),
    content: SizedBox(
      width: 420,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            controller: _name,
            decoration: const InputDecoration(labelText: 'Name bots ask for', hintText: 'mytool'),
          ),
          TextField(
            controller: _label,
            decoration: const InputDecoration(labelText: 'What you call it', hintText: 'My tool'),
          ),
          TextField(
            controller: _path,
            decoration: const InputDecoration(
              labelText: 'Where its sign-in lives',
              hintText: '.config/mytool',
              helperText: 'Relative to your home folder',
            ),
          ),
          DropdownButtonFormField<LendRisk>(
            initialValue: _risk,
            decoration: const InputDecoration(labelText: 'Risk'),
            items: [
              for (final risk in LendRisk.values)
                DropdownMenuItem(value: risk, child: Text(risk.label)),
            ],
            onChanged: (risk) => setState(() => _risk = risk ?? _risk),
          ),
          if (_problem != null)
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: Text(_problem!, style: TextStyle(color: Theme.of(context).colorScheme.error)),
            ),
        ],
      ),
    ),
    actions: [
      TextButton(onPressed: () => Navigator.pop(context), child: const Text('Cancel')),
      FilledButton(onPressed: _submit, child: const Text('Add sign-in')),
    ],
  );
}
