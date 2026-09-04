/// OrgBot — a group chat of AI bots working a shared task board.
///
/// This client connects to a `orgbot serve` WebSocket endpoint, renders the
/// room's chat, roster, and board live, and lets a person talk to the room.
library;

import 'package:flutter/material.dart';

import 'orgbot/credentials.dart';
import 'orgbot/library.dart';
import 'orgbot/model_catalog.dart';
import 'ui/brand.dart';
import 'ui/connect_screen.dart';
import 'ui/sign_in_screen.dart';
import 'ui/orgbot_screen.dart';

void main() {
  runApp(const OrgBotApp());
}


/// Choose: join a hosted room, or run one entirely on this device.
class HomeScreen extends StatelessWidget {
  final Library library;
  final Credentials credentials;
  const HomeScreen(
      {super.key, required this.library, required this.credentials});

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Scaffold(
      body: SafeArea(
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 520),
            child: ListView(
              shrinkWrap: true,
              padding: const EdgeInsets.all(24),
              children: [
                const Center(child: BrandMark(size: 84)),
                const SizedBox(height: 18),
                Text(
                  'OrgBot',
                  textAlign: TextAlign.center,
                  style: Theme.of(context)
                      .textTheme
                      .headlineMedium
                      ?.copyWith(fontWeight: FontWeight.w700),
                ),
                const SizedBox(height: 6),
                Text(
                  'A group chat of AI bots working a shared task board.',
                  textAlign: TextAlign.center,
                  style: TextStyle(color: scheme.onSurfaceVariant, height: 1.4),
                ),
                const SizedBox(height: 32),
                _HomeCard(
                  icon: Icons.smart_toy_outlined,
                  title: 'Bots & rooms',
                  subtitle:
                      'Build your bots, put them in a room, and talk to them '
                      'while they work.',
                  primary: true,
                  onTap: () => Navigator.of(context).push(
                    MaterialPageRoute(
                        builder: (_) => OrgBotScreen(library: library)),
                  ),
                ),
                const SizedBox(height: 12),
                _HomeCard(
                  icon: Icons.key_outlined,
                  title: 'Sign in',
                  subtitle:
                      'Bring your Claude, Codex or Grok subscription to this '
                      'device so your bots can use it.',
                  onTap: () => Navigator.of(context).push(
                    MaterialPageRoute(
                        builder: (_) => SignInScreen(credentials: credentials)),
                  ),
                ),
                const SizedBox(height: 12),
                _HomeCard(
                  icon: Icons.wifi_tethering,
                  title: 'Join a hosted room',
                  subtitle:
                      'Connect to a room someone else is running, over WebSocket.',
                  onTap: () => Navigator.of(context).push(
                    MaterialPageRoute(builder: (_) => const ConnectScreen()),
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

/// One choice on the home screen.
class _HomeCard extends StatelessWidget {
  final IconData icon;
  final String title;
  final String subtitle;
  final bool primary;
  final VoidCallback onTap;

  const _HomeCard({
    required this.icon,
    required this.title,
    required this.subtitle,
    required this.onTap,
    this.primary = false,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Material(
      color: primary ? scheme.primaryContainer : scheme.surfaceContainerHighest,
      borderRadius: BorderRadius.circular(16),
      child: InkWell(
        borderRadius: BorderRadius.circular(16),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(18),
          child: Row(
            children: [
              Icon(icon,
                  size: 28,
                  color: primary ? scheme.onPrimaryContainer : scheme.primary),
              const SizedBox(width: 16),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(title,
                        style: const TextStyle(
                            fontSize: 16, fontWeight: FontWeight.w700)),
                    const SizedBox(height: 4),
                    Text(
                      subtitle,
                      style: TextStyle(
                        fontSize: 13,
                        height: 1.35,
                        color: primary
                            ? scheme.onPrimaryContainer.withValues(alpha: 0.85)
                            : scheme.onSurfaceVariant,
                      ),
                    ),
                  ],
                ),
              ),
              Icon(Icons.chevron_right, color: scheme.onSurfaceVariant),
            ],
          ),
        ),
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
  final Credentials _credentials = Credentials();

  @override
  void initState() {
    super.initState();
    // The model catalog consults these before falling back to local CLIs,
    // which a phone does not have.
    ModelCatalog.credentials = _credentials;
    _credentials.load();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'OrgBot',
      debugShowCheckedModeBanner: false,
      theme: _theme(Brightness.light),
      darkTheme: _theme(Brightness.dark),
      themeMode: ThemeMode.dark,
      home: HomeScreen(library: _library, credentials: _credentials),
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
