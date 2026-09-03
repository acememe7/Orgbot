/// OrgBot — a group chat of AI bots working a shared task board.
///
/// This client connects to a `gptbots serve` WebSocket endpoint, renders the
/// room's chat, roster, and board live, and lets a person talk to the room.
library;

import 'package:flutter/material.dart';

import 'orgbot/library.dart';
import 'ui/connect_screen.dart';
import 'ui/orgbot_screen.dart';

void main() {
  runApp(const OrgBotApp());
}


/// Choose: join a hosted room, or run one entirely on this device.
class HomeScreen extends StatelessWidget {
  final Library library;
  const HomeScreen({super.key, required this.library});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('OrgBot')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          Card(
            child: ListTile(
              leading: const Icon(Icons.smart_toy_outlined),
              title: const Text('Bots & Rooms'),
              subtitle: const Text('Create your bots, open a group chat room, and watch them work.'),
              onTap: () => Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => OrgBotScreen(library: library)),
              ),
            ),
          ),
          Card(
            child: ListTile(
              leading: const Icon(Icons.wifi_tethering),
              title: const Text('Join a hosted room'),
              subtitle: const Text('Connect to a gptbots serve room over WebSocket.'),
              onTap: () => Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => const ConnectScreen()),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The application root.
class OrgBotApp extends StatefulWidget {
  const OrgBotApp({super.key});

  @override
  State<OrgBotApp> createState() => _OrgBotAppState();
}

class _OrgBotAppState extends State<OrgBotApp> {
  final Library _library = Library();

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'OrgBot',
      debugShowCheckedModeBanner: false,
      theme: _theme(Brightness.light),
      darkTheme: _theme(Brightness.dark),
      themeMode: ThemeMode.dark,
      home: HomeScreen(library: _library),
    );
  }

  ThemeData _theme(Brightness brightness) {
    final scheme = ColorScheme.fromSeed(
      seedColor: const Color(0xFF6D5AE6),
      brightness: brightness,
    );
    return ThemeData(
      useMaterial3: true,
      colorScheme: scheme,
      scaffoldBackgroundColor: scheme.surface,
      fontFamily: 'sans-serif',
    );
  }
}
