/// The first screen: point the app at a running room and connect.
library;

import 'package:flutter/material.dart';

import '../net/room_client.dart';
import '../state/room_store.dart';
import 'room_screen.dart';

/// Collects a server URL and a display name, then opens the room.
class ConnectScreen extends StatefulWidget {
  const ConnectScreen({super.key});

  @override
  State<ConnectScreen> createState() => _ConnectScreenState();
}

class _ConnectScreenState extends State<ConnectScreen> {
  final _url = TextEditingController(text: 'ws://127.0.0.1:8787/ws');
  final _name = TextEditingController(text: 'you');
  bool _connecting = false;

  @override
  void dispose() {
    _url.dispose();
    _name.dispose();
    super.dispose();
  }

  Future<void> _connect() async {
    final url = _url.text.trim();
    final name = _name.text.trim().isEmpty ? 'you' : _name.text.trim();
    if (url.isEmpty) return;

    setState(() => _connecting = true);
    final client = RoomClient();
    final store = RoomStore(client);
    await client.connect(url);

    if (!mounted) return;
    setState(() => _connecting = false);

    if (client.current == LinkState.error) {
      await client.close();
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not connect to $url')),
      );
      return;
    }

    await Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => RoomScreen(store: store, client: client, displayName: name),
      ),
    );
    // Back from the room: tear the connection down.
    await client.close();
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Scaffold(
      body: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 420),
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Icon(Icons.forum_outlined, size: 56, color: scheme.primary),
                const SizedBox(height: 16),
                Text(
                  'GPTBots',
                  textAlign: TextAlign.center,
                  style: Theme.of(context).textTheme.headlineMedium,
                ),
                const SizedBox(height: 4),
                Text(
                  'Connect to a running room',
                  textAlign: TextAlign.center,
                  style: TextStyle(color: scheme.onSurfaceVariant),
                ),
                const SizedBox(height: 32),
                TextField(
                  controller: _url,
                  decoration: const InputDecoration(
                    labelText: 'Server URL',
                    hintText: 'ws://host:8787/ws',
                    border: OutlineInputBorder(),
                    prefixIcon: Icon(Icons.link),
                  ),
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: _name,
                  decoration: const InputDecoration(
                    labelText: 'Your name',
                    border: OutlineInputBorder(),
                    prefixIcon: Icon(Icons.person_outline),
                  ),
                  onSubmitted: (_) => _connect(),
                ),
                const SizedBox(height: 24),
                FilledButton.icon(
                  onPressed: _connecting ? null : _connect,
                  icon: _connecting
                      ? const SizedBox(
                          width: 18,
                          height: 18,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.play_arrow),
                  label: Text(_connecting ? 'Connecting…' : 'Connect'),
                  style: FilledButton.styleFrom(
                    padding: const EdgeInsets.symmetric(vertical: 16),
                  ),
                ),
                const SizedBox(height: 16),
                Text(
                  'Start a room with:  gptbots serve room.toml',
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    fontFamily: 'monospace',
                    fontSize: 12,
                    color: scheme.onSurfaceVariant,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
