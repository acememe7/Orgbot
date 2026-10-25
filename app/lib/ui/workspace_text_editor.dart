import 'package:flutter/material.dart';

class WorkspaceTextEditor extends StatelessWidget {
  const WorkspaceTextEditor({
    super.key,
    required this.controller,
    required this.loading,
    required this.saving,
    required this.error,
    required this.dirty,
    required this.onChanged,
    required this.onReload,
  });

  final TextEditingController controller;
  final bool loading;
  final bool saving;
  final String? error;
  final bool dirty;
  final ValueChanged<String> onChanged;
  final VoidCallback onReload;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    if (loading) {
      return const Center(child: CircularProgressIndicator(strokeWidth: 2));
    }
    if (error != null && controller.text.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(error!, textAlign: TextAlign.center),
            const SizedBox(height: 8),
            TextButton.icon(
              onPressed: saving ? null : onReload,
              icon: const Icon(Icons.refresh, size: 16),
              label: const Text('Reload file'),
            ),
          ],
        ),
      );
    }
    return Column(
      children: [
        if (error != null)
          MaterialBanner(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
            content: Text(error!),
            leading: Icon(Icons.warning_amber_outlined, color: scheme.error),
            actions: [
              TextButton(
                onPressed: saving ? null : onReload,
                child: const Text('Reload'),
              ),
            ],
          )
        else if (saving)
          const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: TextField(
            controller: controller,
            onChanged: onChanged,
            enabled: !saving,
            expands: true,
            minLines: null,
            maxLines: null,
            textAlignVertical: TextAlignVertical.top,
            keyboardType: TextInputType.multiline,
            autocorrect: false,
            enableSuggestions: false,
            style: Theme.of(context).textTheme.bodyMedium?.copyWith(
              fontFamily: 'monospace',
              fontSize: 13,
              height: 1.5,
            ),
            decoration: const InputDecoration(
              border: InputBorder.none,
              contentPadding: EdgeInsets.all(14),
              hintText: 'Edit this text file…',
            ),
          ),
        ),
        if (!dirty && !saving && error == null)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
            child: Align(
              alignment: Alignment.centerLeft,
              child: Text(
                'Saved',
                style: Theme.of(context).textTheme.labelSmall?.copyWith(
                  color: scheme.onSurfaceVariant,
                ),
              ),
            ),
          ),
      ],
    );
  }
}
