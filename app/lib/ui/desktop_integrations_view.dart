import 'package:flutter/material.dart';

import '../orgbot/desktop_integrations.dart';

class DesktopIntegrationsView extends StatefulWidget {
  const DesktopIntegrationsView({
    super.key,
    this.homeDirectory,
    @visibleForTesting this.discoverForTesting,
  });

  @visibleForTesting
  final String? homeDirectory;

  @visibleForTesting
  final Future<DesktopIntegrationSnapshot> Function()? discoverForTesting;

  @override
  State<DesktopIntegrationsView> createState() =>
      _DesktopIntegrationsViewState();
}

class _DesktopIntegrationsViewState extends State<DesktopIntegrationsView> {
  late Future<DesktopIntegrationSnapshot> _snapshot;

  @override
  void initState() {
    super.initState();
    _snapshot = _discover();
  }

  Future<DesktopIntegrationSnapshot> _discover() =>
      widget.discoverForTesting?.call() ??
      DesktopIntegrations.discover(homeDirectory: widget.homeDirectory);

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
    required List<Widget> children,
  }) => ExpansionTile(
    initiallyExpanded: false,
    tilePadding: EdgeInsets.zero,
    childrenPadding: EdgeInsets.zero,
    leading: Icon(icon, size: 18),
    title: Row(
      children: [
        Expanded(child: Text(title)),
        Text('$count', style: Theme.of(context).textTheme.labelMedium),
      ],
    ),
    children: children,
  );

  Widget _serverRow(BuildContext context, DesktopMcpStatus server) {
    final scheme = Theme.of(context).colorScheme;
    final stateLabel = switch (server.state) {
      DesktopIntegrationState.configured => 'Configured',
      DesktopIntegrationState.unsupported => 'Unsupported',
      DesktopIntegrationState.unavailable => 'Unavailable',
    };
    final color = server.state == DesktopIntegrationState.configured
        ? scheme.primary
        : scheme.error;
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
              const SizedBox(width: 8),
              Text(
                stateLabel,
                style: Theme.of(
                  context,
                ).textTheme.labelSmall?.copyWith(color: color),
              ),
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
