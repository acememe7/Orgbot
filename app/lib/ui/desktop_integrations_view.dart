import 'package:flutter/material.dart';
import '../orgbot/desktop_integrations.dart';

class DesktopIntegrationsView extends StatefulWidget {
  const DesktopIntegrationsView({super.key});

  @override
  State<DesktopIntegrationsView> createState() =>
      _DesktopIntegrationsViewState();
}

class _DesktopIntegrationsViewState extends State<DesktopIntegrationsView> {
  late Future<DesktopIntegrationSnapshot> _snapshot =
      DesktopIntegrations.discover();

  @override
  Widget build(
    BuildContext context,
  ) => FutureBuilder<DesktopIntegrationSnapshot>(
    future: _snapshot,
    builder: (context, state) => Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SizedBox(height: 24),
        Row(
          children: [
            Expanded(
              child: Text(
                'Tools on this computer',
                style: Theme.of(context).textTheme.titleLarge,
              ),
            ),
            IconButton(
              tooltip: 'Refresh tools',
              onPressed: () =>
                  setState(() => _snapshot = DesktopIntegrations.discover()),
              icon: const Icon(Icons.refresh),
            ),
          ],
        ),
        const Text(
          'Native CLI agents use their own MCP servers, plugins and skills. Compatible configured servers and skill folders are also supplied to API agents when a session starts.',
        ),
        const SizedBox(height: 12),
        if (state.connectionState != ConnectionState.done)
          const LinearProgressIndicator()
        else if (state.hasError)
          Text(
            'Could not inspect integrations: ${state.error}',
            style: TextStyle(color: Theme.of(context).colorScheme.error),
          )
        else if (state.hasData) ...[
          if (state.data!.mcpServers.isEmpty)
            const ListTile(
              leading: Icon(Icons.extension_outlined),
              title: Text('No direct MCP servers found'),
              subtitle: Text(
                'Configure a server in your Codex, Claude or Grok CLI, then refresh.',
              ),
            ),
          for (final server in state.data!.mcpServers)
            ListTile(
              contentPadding: EdgeInsets.zero,
              leading: Icon(
                server.state == DesktopIntegrationState.configured
                    ? Icons.extension_outlined
                    : Icons.info_outline,
              ),
              title: Text(
                server.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              subtitle: Text('${server.source.name} · ${server.detail}'),
              trailing: Text(
                server.state.name,
                style: const TextStyle(fontSize: 12),
              ),
            ),
          ExpansionTile(
            tilePadding: EdgeInsets.zero,
            title: Text('${state.data!.skills.length} skill folders'),
            children: [
              for (final skill in state.data!.skills)
                ListTile(
                  dense: true,
                  title: Text(
                    skill.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                  subtitle: SelectableText(skill.path),
                ),
            ],
          ),
        ],
      ],
    ),
  );
}
