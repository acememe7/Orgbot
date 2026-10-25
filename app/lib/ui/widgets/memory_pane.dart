import 'package:flutter/material.dart';

import '../../state/engine_room_controller.dart';

/// A project-scoped note shown in the owner memory panel.
@immutable
class ProjectMemoryItem {
  const ProjectMemoryItem({
    required this.id,
    required this.title,
    required this.content,
    required this.shared,
    this.sourceRoom,
    this.sourceTask,
  });

  final String id;
  final String title;
  final String content;
  final bool shared;
  final String? sourceRoom;
  final String? sourceTask;
}

/// A semantic search result in the active project's memory.
@immutable
class ProjectMemoryHit {
  const ProjectMemoryHit({required this.item, required this.score});

  final ProjectMemoryItem item;
  final double score;
}

/// Owner controls for durable project memory.
///
/// Persistence and scope authorization stay behind callbacks supplied by the
/// local room client; this widget never reads a path or reaches another
/// project's store directly.
class MemoryPane extends StatefulWidget {
  const MemoryPane({
    super.key,
    required this.entries,
    required this.onSetShared,
    required this.onDelete,
    required this.onShareToProject,
    required this.onSearch,
    this.searchResults = const [],
    this.loading = false,
    this.error,
    this.onRefresh,
  });

  final List<ProjectMemoryItem> entries;
  final List<ProjectMemoryHit> searchResults;
  final bool loading;
  final String? error;
  final Future<void> Function(String id, bool shared) onSetShared;
  final Future<void> Function(String id) onDelete;
  final Future<void> Function(String id, String targetProject) onShareToProject;
  final Future<void> Function(String query) onSearch;
  final Future<void> Function()? onRefresh;

  @override
  State<MemoryPane> createState() => _MemoryPaneState();
}

/// Stateful adapter between the room's authenticated control channel and the
/// reusable owner memory pane.
class ProjectMemoryPanel extends StatefulWidget {
  const ProjectMemoryPanel({super.key, required this.controller});

  final EngineRoomController controller;

  @override
  State<ProjectMemoryPanel> createState() => _ProjectMemoryPanelState();
}

class _ProjectMemoryPanelState extends State<ProjectMemoryPanel> {
  List<ProjectMemoryItem> _entries = const [];
  List<ProjectMemoryHit> _hits = const [];
  bool _loading = true;
  String? _error;

  @override
  void initState() {
    super.initState();
    _refresh();
  }

  @override
  void didUpdateWidget(covariant ProjectMemoryPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) _refresh();
  }

  ProjectMemoryItem _item(Map<String, Object?> map) => ProjectMemoryItem(
    id: map['id'] as String? ?? '',
    title: map['title'] as String? ?? 'Saved memory',
    content: map['content'] as String? ?? '',
    shared: map['shared'] as bool? ?? false,
    sourceRoom: map['source_room'] as String?,
    sourceTask: map['source_task'] as String?,
  );

  Future<void> _run(
    Future<List<Map<String, Object?>>> Function() action,
  ) async {
    if (!mounted) return;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final maps = await action();
      if (!mounted) return;
      setState(() {
        _entries = maps.map(_item).toList(growable: false);
        _loading = false;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _error = error.toString();
        _loading = false;
      });
    }
  }

  Future<void> _refresh() => _run(widget.controller.listProjectMemory);

  Future<void> _search(String query) async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final maps = await widget.controller.searchProjectMemory(query);
      if (!mounted) return;
      setState(() {
        _hits = maps
            .map((map) {
              final item = map['item'];
              if (item is! Map) return null;
              return ProjectMemoryHit(
                item: _item(item.cast<String, Object?>()),
                score: (map['score'] as num?)?.toDouble() ?? 0,
              );
            })
            .whereType<ProjectMemoryHit>()
            .toList(growable: false);
        _loading = false;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _error = error.toString();
        _loading = false;
      });
    }
  }

  Future<void> _updated(Future<List<Map<String, Object?>>> Function() action) =>
      _run(action);

  @override
  Widget build(BuildContext context) => MemoryPane(
    entries: _entries,
    searchResults: _hits,
    loading: _loading,
    error: _error,
    onRefresh: _refresh,
    onSearch: _search,
    onSetShared: (id, shared) =>
        _updated(() => widget.controller.setProjectMemoryShared(id, shared)),
    onDelete: (id) => _updated(() => widget.controller.deleteProjectMemory(id)),
    onShareToProject: (id, project) =>
        _updated(() => widget.controller.shareProjectMemory(id, project)),
  );
}

class _MemoryPaneState extends State<MemoryPane> {
  final _query = TextEditingController();
  bool _searching = false;

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ListView(
      padding: const EdgeInsets.fromLTRB(20, 20, 20, 28),
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                'Project memory',
                style: theme.textTheme.headlineSmall,
              ),
            ),
            if (widget.onRefresh != null)
              IconButton(
                tooltip: 'Refresh project memory',
                onPressed: widget.loading ? null : widget.onRefresh,
                icon: const Icon(Icons.refresh),
              ),
          ],
        ),
        const SizedBox(height: 6),
        Text(
          'Completed work and saved decisions stay available to rooms in this project. Choose what to share elsewhere or remove.',
          style: theme.textTheme.bodyMedium?.copyWith(
            color: theme.colorScheme.onSurfaceVariant,
          ),
        ),
        const SizedBox(height: 20),
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _query,
                textInputAction: TextInputAction.search,
                onSubmitted: (_) => _runSearch(),
                decoration: const InputDecoration(
                  labelText: 'Search project memory',
                  hintText: 'Describe the decision or outcome',
                  prefixIcon: Icon(Icons.search),
                  border: OutlineInputBorder(),
                ),
              ),
            ),
            const SizedBox(width: 8),
            FilledButton.tonalIcon(
              onPressed: _searching ? null : _runSearch,
              icon: _searching
                  ? const SizedBox.square(
                      dimension: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.arrow_forward),
              label: const Text('Search'),
            ),
          ],
        ),
        if (widget.error != null) ...[
          const SizedBox(height: 12),
          Semantics(
            liveRegion: true,
            child: Text(
              widget.error!,
              style: TextStyle(color: theme.colorScheme.error),
            ),
          ),
        ],
        if (widget.loading) ...[
          const SizedBox(height: 24),
          const Center(child: CircularProgressIndicator()),
        ] else ...[
          if (widget.searchResults.isNotEmpty) ...[
            const SizedBox(height: 24),
            Text('Search results', style: theme.textTheme.titleMedium),
            const SizedBox(height: 8),
            for (final hit in widget.searchResults)
              _MemoryCard(item: hit.item, score: hit.score),
          ],
          const SizedBox(height: 24),
          Row(
            children: [
              Expanded(
                child: Text(
                  'Saved knowledge',
                  style: theme.textTheme.titleMedium,
                ),
              ),
              Text(
                '${widget.entries.length}',
                style: theme.textTheme.labelLarge,
              ),
            ],
          ),
          const SizedBox(height: 8),
          if (widget.entries.isEmpty)
            const _EmptyMemory()
          else
            for (final item in widget.entries)
              _MemoryCard(
                item: item,
                onSetShared: (value) => widget.onSetShared(item.id, value),
                onDelete: () => _confirmDelete(context, item),
                onShare: () => _share(context, item),
              ),
        ],
      ],
    );
  }

  Future<void> _runSearch() async {
    final query = _query.text.trim();
    if (query.isEmpty) return;
    setState(() => _searching = true);
    try {
      await widget.onSearch(query);
    } finally {
      if (mounted) setState(() => _searching = false);
    }
  }

  Future<void> _confirmDelete(
    BuildContext context,
    ProjectMemoryItem item,
  ) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Delete this memory?'),
        content: Text(
          '“${item.title}” will be removed from this project. This cannot be undone.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Keep'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Delete'),
          ),
        ],
      ),
    );
    if (confirmed == true) await widget.onDelete(item.id);
  }

  Future<void> _share(BuildContext context, ProjectMemoryItem item) async {
    var project = '';
    final target = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Share to another project'),
        content: TextField(
          autofocus: true,
          onChanged: (value) => project = value,
          decoration: const InputDecoration(labelText: 'Target project ID'),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, project.trim()),
            child: const Text('Share copy'),
          ),
        ],
      ),
    );
    if (target != null && target.isNotEmpty) {
      await widget.onShareToProject(item.id, target);
    }
  }
}

class _MemoryCard extends StatelessWidget {
  const _MemoryCard({
    required this.item,
    this.score,
    this.onSetShared,
    this.onDelete,
    this.onShare,
  });

  final ProjectMemoryItem item;
  final double? score;
  final ValueChanged<bool>? onSetShared;
  final VoidCallback? onDelete;
  final VoidCallback? onShare;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 14, 12, 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: Text(item.title, style: theme.textTheme.titleSmall),
                ),
                if (score != null)
                  Text(
                    '${(score! * 100).round()}%',
                    style: theme.textTheme.labelMedium,
                  ),
              ],
            ),
            const SizedBox(height: 6),
            SelectableText(item.content, style: theme.textTheme.bodyMedium),
            if (item.sourceRoom != null || item.sourceTask != null) ...[
              const SizedBox(height: 8),
              Text(
                [
                  if (item.sourceRoom != null) item.sourceRoom!,
                  if (item.sourceTask != null) item.sourceTask!,
                ].join(' · '),
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ],
            if (onSetShared != null || onDelete != null || onShare != null) ...[
              const Divider(height: 20),
              Wrap(
                alignment: WrapAlignment.spaceBetween,
                crossAxisAlignment: WrapCrossAlignment.center,
                spacing: 8,
                runSpacing: 4,
                children: [
                  if (onSetShared != null)
                    Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Switch(value: item.shared, onChanged: onSetShared),
                        const Text('Available to project rooms'),
                      ],
                    ),
                  if (onShare != null)
                    TextButton.icon(
                      onPressed: onShare,
                      icon: const Icon(Icons.ios_share),
                      label: const Text('Share copy'),
                    ),
                  if (onDelete != null)
                    IconButton(
                      tooltip: 'Delete memory',
                      onPressed: onDelete,
                      icon: const Icon(Icons.delete_outline),
                    ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _EmptyMemory extends StatelessWidget {
  const _EmptyMemory();

  @override
  Widget build(BuildContext context) => Card(
    child: Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        children: [
          Icon(
            Icons.auto_awesome_outlined,
            size: 28,
            color: Theme.of(context).colorScheme.primary,
          ),
          const SizedBox(height: 10),
          Text(
            'No project memory yet',
            style: Theme.of(context).textTheme.titleSmall,
          ),
          const SizedBox(height: 4),
          const Text('Completed tasks and saved decisions will appear here.'),
        ],
      ),
    ),
  );
}
