import 'package:flutter/material.dart';

import '../workspace/desktop_files.dart';
import '../workspace/trash_settings.dart';

class TrashStorageSettings extends StatefulWidget {
  const TrashStorageSettings({super.key, this.client});

  final TrashSettingsClient? client;

  @override
  State<TrashStorageSettings> createState() => _TrashStorageSettingsState();
}

class _TrashStorageSettingsState extends State<TrashStorageSettings> {
  late final TrashSettingsClient _client;
  final _path = TextEditingController();
  TrashSettings? _settings;
  String? _error;
  bool _loading = false;
  bool _saving = false;
  bool _browsing = false;

  bool get _busy => _loading || _saving || _browsing;

  @override
  void initState() {
    super.initState();
    _client = widget.client ?? const TrashSettingsClient();
    _load();
  }

  @override
  void dispose() {
    _path.dispose();
    super.dispose();
  }

  String _message(Object error) => error is FormatException
      ? error.message.toString()
      : error.toString().replaceFirst('Exception: ', '');

  Future<void> _load() async {
    if (_busy) return;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final settings = await _client.read();
      if (!mounted) return;
      setState(() {
        _settings = settings;
        _path.text = settings.root;
      });
    } catch (error) {
      if (mounted) setState(() => _error = _message(error));
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _browse() async {
    if (_busy) return;
    setState(() {
      _browsing = true;
      _error = null;
    });
    try {
      final selected = await DesktopFiles.chooseProjectFolder();
      if (mounted && selected != null && selected.isNotEmpty) {
        setState(() => _path.text = selected);
      }
    } catch (error) {
      if (mounted) setState(() => _error = _message(error));
    } finally {
      if (mounted) setState(() => _browsing = false);
    }
  }

  Future<void> _save() async {
    if (_busy) return;
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      final settings = await _client.configure(_path.text);
      if (!mounted) return;
      setState(() {
        _settings = settings;
        _path.text = settings.root;
      });
    } catch (error) {
      if (mounted) setState(() => _error = _message(error));
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('File recovery', style: theme.textTheme.titleMedium),
            const SizedBox(height: 6),
            Text(
              'Choose where deleted project files are kept. When automatic cleanup is configured, deleted files remain available for seven full days.',
              style: theme.textTheme.bodyMedium,
            ),
            if (_settings?.requiresSelection == true) ...[
              const SizedBox(height: 12),
              const Text(
                'This installation needs you to choose a recovery folder. The suggested folder below is not active until you save it.',
                key: ValueKey('trash-selection-needed'),
              ),
            ],
            if (_loading) ...[
              const SizedBox(height: 14),
              const LinearProgressIndicator(),
              const SizedBox(height: 6),
              const Text('Checking recovery folder…'),
            ] else ...[
              const SizedBox(height: 12),
              TextField(
                key: const ValueKey('trash-root-field'),
                controller: _path,
                enabled: !_busy,
                keyboardType: TextInputType.streetAddress,
                autocorrect: false,
                enableSuggestions: false,
                decoration: const InputDecoration(
                  labelText: 'Recovery folder',
                  hintText: '/home/you/OrgBot Trash',
                  border: OutlineInputBorder(),
                ),
                onSubmitted: _busy ? null : (_) => _save(),
              ),
              const SizedBox(height: 10),
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  OutlinedButton.icon(
                    key: const ValueKey('trash-browse'),
                    onPressed: _busy ? null : _browse,
                    icon: _browsing
                        ? const SizedBox.square(
                            dimension: 16,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.folder_open_outlined),
                    label: Text(_browsing ? 'Opening…' : 'Browse'),
                  ),
                  FilledButton.icon(
                    key: const ValueKey('trash-save'),
                    onPressed: _busy ? null : _save,
                    icon: _saving
                        ? const SizedBox.square(
                            dimension: 16,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.save_outlined),
                    label: Text(_saving ? 'Saving…' : 'Save folder'),
                  ),
                  if (_error != null)
                    TextButton.icon(
                      key: const ValueKey('trash-retry'),
                      onPressed: _busy ? null : _load,
                      icon: const Icon(Icons.refresh),
                      label: const Text('Retry'),
                    ),
                ],
              ),
              if (_error != null) ...[
                const SizedBox(height: 8),
                Semantics(
                  liveRegion: true,
                  child: Text(
                    _error!,
                    key: const ValueKey('trash-error'),
                    style: TextStyle(color: theme.colorScheme.error),
                  ),
                ),
              ],
              if (_settings != null && !_settings!.requiresSelection) ...[
                const SizedBox(height: 8),
                Text('Current folder: ${_settings!.root}'),
              ],
            ],
          ],
        ),
      ),
    );
  }
}
