import 'package:flutter/material.dart';

import '../orgbot/desktop_integrations.dart';
import '../orgbot/integration_registry.dart';

class DesktopIntegrationsView extends StatefulWidget {
  const DesktopIntegrationsView({
    super.key,
    this.homeDirectory,
    this.registry,
    @visibleForTesting this.discoverForTesting,
  });

  @visibleForTesting
  final String? homeDirectory;

  @visibleForTesting
  final IntegrationRegistry? registry;

  @visibleForTesting
  final Future<DesktopIntegrationSnapshot> Function()? discoverForTesting;

  @override
  State<DesktopIntegrationsView> createState() =>
      _DesktopIntegrationsViewState();
}

class _DesktopIntegrationsViewState extends State<DesktopIntegrationsView> {
  late Future<DesktopIntegrationSnapshot> _snapshot;
  final Set<String> _testing = {};
  late final IntegrationRegistry _registry =
      widget.registry ??
      IntegrationRegistry(
        supportDirectory: widget.homeDirectory == null
            ? null
            : '${widget.homeDirectory}/.local/share/orgbot',
      );

  @override
  void initState() {
    super.initState();
    _snapshot = _discover();
  }

  Future<DesktopIntegrationSnapshot> _discover() =>
      widget.discoverForTesting?.call() ??
      DesktopIntegrations.discover(
        homeDirectory: widget.homeDirectory,
        registry: _registry,
      );

  void _refresh() {
    setState(() {
      _snapshot = _discover();
    });
  }

  @override
  Widget build(
    BuildContext context,
  ) => FutureBuilder<DesktopIntegrationSnapshot>(
    future: _snapshot,
    builder: (context, state) {
      final data = state.data;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const SizedBox(height: 14),
          Row(
            children: [
              Expanded(
                child: Text(
                  'Tools on this computer',
                  style: Theme.of(context).textTheme.titleMedium,
                ),
              ),
              IconButton(
                tooltip: 'Refresh tools',
                onPressed: state.connectionState == ConnectionState.waiting
                    ? null
                    : _refresh,
                icon: const Icon(Icons.refresh),
              ),
              IconButton(
                tooltip: 'Add MCP server',
                onPressed: () => _editServer(),
                icon: const Icon(Icons.add_link),
              ),
              IconButton(
                tooltip: 'Manage provider plugins',
                onPressed: _managePlugins,
                icon: const Icon(Icons.extension),
              ),
            ],
          ),
          if (state.connectionState == ConnectionState.waiting)
            const LinearProgressIndicator(minHeight: 2)
          else if (state.hasError)
            _problem(context, 'Could not inspect integrations: ${state.error}')
          else if (data != null) ...[
            _section(
              context,
              title: 'MCP servers',
              count: data.mcpServers.length,
              icon: Icons.hub_outlined,
              children: data.mcpServers.isEmpty
                  ? const [_EmptyIntegrationRow('No MCP servers configured')]
                  : [
                      SizedBox(
                        height: _listHeight(data.mcpServers.length, 58, 290),
                        child: ListView.builder(
                          padding: EdgeInsets.zero,
                          itemCount: data.mcpServers.length,
                          itemBuilder: (context, index) =>
                              _serverRow(context, data.mcpServers[index]),
                        ),
                      ),
                    ],
            ),
            _section(
              context,
              title: 'Skills',
              count: data.skills.length,
              icon: Icons.auto_awesome_outlined,
              trailingAction: IconButton(
                tooltip: 'Install skill',
                onPressed: _installSkill,
                icon: const Icon(Icons.add),
              ),
              children: data.skills.isEmpty
                  ? const [_EmptyIntegrationRow('No skill folders found')]
                  : [
                      SizedBox(
                        height: _listHeight(data.skills.length, 56, 320),
                        child: ListView.builder(
                          padding: EdgeInsets.zero,
                          itemCount: data.skills.length,
                          itemBuilder: (context, index) =>
                              _skillRow(context, data.skills[index]),
                        ),
                      ),
                    ],
            ),
          ],
        ],
      );
    },
  );

  static double _listHeight(int count, double rowHeight, double maxHeight) =>
      (count * rowHeight).clamp(rowHeight, maxHeight).toDouble();

  Widget _section(
    BuildContext context, {
    required String title,
    required int count,
    required IconData icon,
    Widget? trailingAction,
    required List<Widget> children,
  }) => ExpansionTile(
    initiallyExpanded: false,
    tilePadding: EdgeInsets.zero,
    childrenPadding: EdgeInsets.zero,
    leading: Icon(icon, size: 18),
    title: Row(
      children: [
        Expanded(child: Text(title)),
        ?trailingAction,
        Text('$count', style: Theme.of(context).textTheme.labelMedium),
      ],
    ),
    children: children,
  );

  Widget _serverRow(BuildContext context, DesktopMcpStatus server) {
    final scheme = Theme.of(context).colorScheme;
    final stateLabel = switch (server.state) {
      DesktopIntegrationState.configured => 'Configured',
      DesktopIntegrationState.disabled => 'Disabled',
      DesktopIntegrationState.unsupported => 'Unsupported',
      DesktopIntegrationState.unavailable => 'Unavailable',
    };
    final color = switch (server.state) {
      DesktopIntegrationState.configured => scheme.primary,
      DesktopIntegrationState.disabled => scheme.onSurfaceVariant,
      DesktopIntegrationState.unsupported ||
      DesktopIntegrationState.unavailable => scheme.error,
    };
    return Tooltip(
      message:
          '${server.source.name} · ${server.transport.name}\n${server.detail}',
      child: SizedBox(
        height: 58,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
          child: Row(
            children: [
              Icon(Icons.extension_outlined, size: 17, color: color),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Text(
                      server.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                    Text(
                      '${server.source.name} · ${server.detail}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                        color: scheme.onSurfaceVariant,
                      ),
                    ),
                  ],
                ),
              ),
              if (server.managed) ...[
                PopupMenuButton<String>(
                  tooltip: 'Manage ${server.name}',
                  onSelected: (action) => _serverAction(server, action),
                  itemBuilder: (context) => [
                    PopupMenuItem(
                      value: 'test',
                      enabled: !_testing.contains(server.managedId),
                      child: const Text('Test connection'),
                    ),
                    const PopupMenuItem(value: 'edit', child: Text('Edit')),
                    const PopupMenuItem(value: 'remove', child: Text('Remove')),
                  ],
                ),
                Switch.adaptive(
                  value: server.state != DesktopIntegrationState.disabled,
                  onChanged: (value) => _setEnabled(server, value),
                  materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                ),
              ] else ...[
                const SizedBox(width: 8),
                Text(
                  stateLabel,
                  style: Theme.of(
                    context,
                  ).textTheme.labelSmall?.copyWith(color: color),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }

  Widget _skillRow(BuildContext context, DesktopSkillStatus skill) => Tooltip(
    message: '${skill.source.name} · ${skill.path}',
    child: SizedBox(
      height: 56,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 5),
        child: Row(
          children: [
            const Icon(Icons.auto_awesome_outlined, size: 17),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Text(
                    skill.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                  SelectableText(
                    skill.path,
                    maxLines: 1,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ],
              ),
            ),
            if (skill.source == DesktopIntegrationSource.managed)
              IconButton(
                tooltip: 'Remove ${skill.name}',
                onPressed: () => _removeSkill(skill.name),
                icon: const Icon(Icons.delete_outline, size: 18),
              ),
          ],
        ),
      ),
    ),
  );

  Widget _problem(BuildContext context, String message) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 12),
    child: Row(
      children: [
        Expanded(
          child: Text(
            message,
            style: TextStyle(color: Theme.of(context).colorScheme.error),
          ),
        ),
        TextButton.icon(
          onPressed: _refresh,
          icon: const Icon(Icons.refresh, size: 16),
          label: const Text('Retry'),
        ),
      ],
    ),
  );

  Future<void> _setEnabled(DesktopMcpStatus status, bool enabled) async {
    final id = status.managedId;
    if (id == null) return;
    try {
      await _registry.setMcpEnabled(id, enabled);
      _refresh();
    } catch (_) {
      _message('Could not update the MCP server.');
    }
  }

  Future<void> _manageServer(DesktopMcpStatus status) async {
    final id = status.managedId;
    if (id == null) return;
    final entries = await _registry.listMcp();
    final matching = entries.where((entry) => entry.id == id);
    if (matching.isNotEmpty) await _editServer(server: matching.first);
  }

  Future<void> _serverAction(DesktopMcpStatus status, String action) async {
    switch (action) {
      case 'test':
        final id = status.managedId;
        if (id == null) return;
        setState(() => _testing.add(id));
        try {
          final servers = await _registry.listMcp();
          final matching = servers.where((entry) => entry.id == id);
          if (matching.isEmpty) return;
          _message(
            await DesktopIntegrations.testManaged(matching.first, _registry),
          );
        } catch (_) {
          _message('Could not test the MCP server.');
        } finally {
          if (mounted) setState(() => _testing.remove(id));
        }
      case 'edit':
        await _manageServer(status);
      case 'remove':
        await _removeMcp(status);
    }
  }

  Future<void> _editServer({ManagedMcpServer? server}) async {
    final result = await showDialog<_McpEditResult>(
      context: context,
      builder: (context) => _McpEditor(server: server),
    );
    if (result == null) return;
    try {
      if (server == null &&
          (await _registry.listMcp()).any(
            (entry) => entry.id == result.server.id,
          )) {
        throw ArgumentError('An MCP server with that name already exists.');
      }
      await _registry.saveMcp(result.server, secrets: result.secrets);
      _refresh();
    } catch (error) {
      _message(
        error is ArgumentError
            ? error.message.toString()
            : 'Could not save MCP server.',
      );
    }
  }

  Future<void> _removeMcp(DesktopMcpStatus status) async {
    final id = status.managedId;
    if (id == null) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Remove ${status.name}?'),
        content: const Text('Its saved MCP credentials will also be removed.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    try {
      await _registry.removeMcp(id);
      _refresh();
    } catch (_) {
      _message('Could not remove the MCP server.');
    }
  }

  Future<void> _installSkill() async {
    final controller = TextEditingController();
    final path = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Install a skill'),
        content: TextField(
          controller: controller,
          autofocus: true,
          decoration: const InputDecoration(labelText: 'Folder or ZIP path'),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, controller.text.trim()),
            child: const Text('Install'),
          ),
        ],
      ),
    );
    controller.dispose();
    if (path == null || path.isEmpty) return;
    try {
      if (path.toLowerCase().endsWith('.zip')) {
        await _registry.installSkillArchive(path);
      } else {
        await _registry.installSkillDirectory(path);
      }
      _refresh();
    } catch (error) {
      _message(
        error is ArgumentError ||
                error is FormatException ||
                error is StateError
            ? error.toString()
            : 'Could not install that skill.',
      );
    }
  }

  Future<void> _managePlugins() async {
    final messenger = ScaffoldMessenger.of(context);
    final providers = await ProviderPluginManager().discover(
      home: widget.homeDirectory,
    );
    if (!mounted) return;
    if (providers.isEmpty) {
      _message('No installed CLI reports supported plugin commands.');
      return;
    }
    final plugin = TextEditingController();
    var busy = false;
    await showDialog<void>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setDialogState) => AlertDialog(
          title: const Text('Provider plugins'),
          content: SizedBox(
            width: 500,
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  TextField(
                    controller: plugin,
                    autofocus: true,
                    onChanged: (_) => setDialogState(() {}),
                    decoration: const InputDecoration(
                      labelText: 'Plugin name or marketplace ID',
                    ),
                  ),
                  if (busy) const LinearProgressIndicator(minHeight: 2),
                  for (final provider in providers) ...[
                    const SizedBox(height: 18),
                    Text(
                      provider.provider.label,
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                    if (provider.problem != null) Text(provider.problem!),
                    Wrap(
                      spacing: 8,
                      children: [
                        for (final action in ProviderPluginAction.values)
                          if (provider.actions.contains(action))
                            OutlinedButton(
                              onPressed: busy || plugin.text.trim().isEmpty
                                  ? null
                                  : () async {
                                      setDialogState(() => busy = true);
                                      try {
                                        await ProviderPluginManager().run(
                                          provider,
                                          action,
                                          plugin.text,
                                        );
                                        if (mounted) {
                                          messenger.showSnackBar(
                                            SnackBar(
                                              content: Text(
                                                '${provider.provider.label} plugin command completed.',
                                              ),
                                            ),
                                          );
                                        }
                                      } catch (error) {
                                        if (mounted) {
                                          messenger.showSnackBar(
                                            SnackBar(
                                              content: Text(
                                                error is ArgumentError
                                                    ? error.message.toString()
                                                    : 'The provider CLI could not complete that command.',
                                              ),
                                            ),
                                          );
                                        }
                                      } finally {
                                        setDialogState(() => busy = false);
                                      }
                                    },
                              child: Text(switch (action) {
                                ProviderPluginAction.install => 'Install',
                                ProviderPluginAction.update => 'Update',
                                ProviderPluginAction.remove => 'Remove',
                              }),
                            ),
                      ],
                    ),
                  ],
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: busy ? null : () => Navigator.pop(context),
              child: const Text('Done'),
            ),
          ],
        ),
      ),
    );
    plugin.dispose();
  }

  Future<void> _removeSkill(String name) async {
    try {
      await _registry.removeSkill(name);
      _refresh();
    } catch (_) {
      _message('Could not remove the managed skill.');
    }
  }

  void _message(String message) {
    if (mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(message)));
    }
  }
}

class _McpEditResult {
  const _McpEditResult(this.server, this.secrets);
  final ManagedMcpServer server;
  final Map<String, String> secrets;
}

class _McpEditor extends StatefulWidget {
  const _McpEditor({this.server});
  final ManagedMcpServer? server;

  @override
  State<_McpEditor> createState() => _McpEditorState();
}

class _McpEditorState extends State<_McpEditor> {
  late final _name = TextEditingController(text: widget.server?.name ?? '');
  late final _command = TextEditingController(
    text: widget.server?.command ?? '',
  );
  late final _args = TextEditingController(
    text: widget.server?.args.join('\n') ?? '',
  );
  late final _url = TextEditingController(text: widget.server?.url ?? '');
  late final _env = TextEditingController(
    text: _pairs(widget.server?.env ?? const {}),
  );
  late final _headers = TextEditingController(
    text: _pairs(widget.server?.headers ?? const {}),
  );
  late final _secrets = TextEditingController();
  var _transport = ManagedMcpTransport.stdio;
  String? _error;

  @override
  void initState() {
    super.initState();
    _transport = widget.server?.transport ?? ManagedMcpTransport.stdio;
  }

  @override
  void dispose() {
    for (final controller in [
      _name,
      _command,
      _args,
      _url,
      _env,
      _headers,
      _secrets,
    ]) {
      controller.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(widget.server == null ? 'Add MCP server' : 'Edit MCP server'),
    content: SizedBox(
      width: 480,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: _name,
              autofocus: true,
              decoration: const InputDecoration(labelText: 'Name'),
            ),
            DropdownButtonFormField<ManagedMcpTransport>(
              initialValue: _transport,
              items: const [
                DropdownMenuItem(
                  value: ManagedMcpTransport.stdio,
                  child: Text('Local process (stdio)'),
                ),
                DropdownMenuItem(
                  value: ManagedMcpTransport.http,
                  child: Text('Streamable HTTP'),
                ),
              ],
              onChanged: (value) =>
                  setState(() => _transport = value ?? _transport),
            ),
            if (_transport == ManagedMcpTransport.stdio) ...[
              TextField(
                controller: _command,
                decoration: const InputDecoration(labelText: 'Command'),
              ),
              TextField(
                controller: _args,
                minLines: 1,
                maxLines: 4,
                decoration: const InputDecoration(
                  labelText: 'Arguments, one per line',
                ),
              ),
              TextField(
                controller: _env,
                minLines: 1,
                maxLines: 3,
                decoration: const InputDecoration(
                  labelText: 'Environment references: KEY=REFERENCE',
                ),
              ),
            ] else ...[
              TextField(
                controller: _url,
                decoration: const InputDecoration(labelText: 'HTTP URL'),
              ),
              TextField(
                controller: _headers,
                minLines: 1,
                maxLines: 3,
                decoration: const InputDecoration(
                  labelText: 'Header references: HEADER=REFERENCE',
                ),
              ),
            ],
            TextField(
              controller: _secrets,
              minLines: 1,
              maxLines: 3,
              obscureText: true,
              decoration: const InputDecoration(
                labelText: 'Credentials: REFERENCE=value (empty clears)',
              ),
            ),
            if (_error != null)
              Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  _error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ),
          ],
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(onPressed: _save, child: const Text('Save')),
    ],
  );

  void _save() {
    try {
      final name = _name.text.trim();
      final server = ManagedMcpServer(
        id: widget.server?.id ?? _slug(name),
        name: name,
        enabled: widget.server?.enabled ?? true,
        transport: _transport,
        command: _command.text.trim(),
        args: _args.text
            .split('\n')
            .map((value) => value.trim())
            .where((value) => value.isNotEmpty)
            .toList(),
        url: _url.text.trim(),
        env: _readPairs(_env.text),
        headers: _readPairs(_headers.text),
      );
      Navigator.pop(context, _McpEditResult(server, _readPairs(_secrets.text)));
    } on ArgumentError catch (error) {
      setState(() => _error = error.message.toString());
    }
  }

  static Map<String, String> _readPairs(String text) => Map.fromEntries(
    text
        .split('\n')
        .map((line) => line.trim())
        .where((line) => line.isNotEmpty)
        .map((line) {
          final index = line.indexOf('=');
          if (index < 1) throw ArgumentError('Use KEY=VALUE on each line.');
          return MapEntry(
            line.substring(0, index).trim(),
            line.substring(index + 1).trim(),
          );
        }),
  );

  static String _pairs(Map<String, String> values) =>
      values.entries.map((entry) => '${entry.key}=${entry.value}').join('\n');
  static String _slug(String value) => value
      .toLowerCase()
      .replaceAll(RegExp(r'[^a-z0-9_-]+'), '_')
      .replaceAll(RegExp(r'^_+|_+$'), '');
}

class _EmptyIntegrationRow extends StatelessWidget {
  const _EmptyIntegrationRow(this.message);

  final String message;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 12),
    child: Align(
      alignment: Alignment.centerLeft,
      child: Text(
        message,
        style: Theme.of(context).textTheme.bodySmall?.copyWith(
          color: Theme.of(context).colorScheme.onSurfaceVariant,
        ),
      ),
    ),
  );
}
