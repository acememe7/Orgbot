/// GPTBots — a group chat of AI bots working a shared task board.
///
/// This client connects to a `gptbots serve` WebSocket endpoint, renders the
/// room's chat, roster, and board live, and lets a person talk to the room.
library;

import 'package:flutter/material.dart';

import 'ui/connect_screen.dart';

void main() {
  runApp(const GptBotsApp());
}

/// The application root.
class GptBotsApp extends StatelessWidget {
  const GptBotsApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'GPTBots',
      debugShowCheckedModeBanner: false,
      theme: _theme(Brightness.light),
      darkTheme: _theme(Brightness.dark),
      themeMode: ThemeMode.dark,
      home: const ConnectScreen(),
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
