/// OrgBot — a group chat of AI bots working a shared task board.
///
/// This client connects to a `gptbots serve` WebSocket endpoint, renders the
/// room's chat, roster, and board live, and lets a person talk to the room.
library;

import 'package:flutter/material.dart';

import 'ui/connect_screen.dart';
import 'ui/local_room_screen.dart';

void main() {
  runApp(const OrgBotApp());
}


/// Choose: join a hosted room, or run one entirely on this device.
class HomeScreen extends StatelessWidget {
  const HomeScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('GPTBots')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          Card(
            child: ListTile(
              leading: const Icon(Icons.dns_outlined),
              title: const Text('Run a room on this device'),
              subtitle: const Text('Bots work a task board here — no server, no PC.'),
              onTap: () => Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => const LocalRoomScreen()),
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
class OrgBotApp extends StatelessWidget {
  const OrgBotApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'OrgBot',
      debugShowCheckedModeBanner: false,
      theme: _theme(Brightness.light),
      darkTheme: _theme(Brightness.dark),
      themeMode: ThemeMode.dark,
      home: const HomeScreen(),
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
