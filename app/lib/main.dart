import 'state/room_store.dart';
import 'dart:async';

import 'package:flutter/material.dart';
import 'ui/polish.dart';
import 'engine/agent_cli.dart';
import 'orgbot/credentials.dart';
import 'orgbot/library.dart';
import 'orgbot/model_catalog.dart';
import 'state/background_rooms.dart';
import 'ui/orgbot_screen.dart';

void main() => runApp(const OrgBotApp());

ThemeData orgBotTheme(Brightness brightness, {bool desktop = false}) {
  final dark = brightness == Brightness.dark;
  var scheme = ColorScheme.fromSeed(
    seedColor: const Color(0xFF65B8AB),
    brightness: brightness,
    surface: dark ? const Color(0xFF101214) : const Color(0xFFF5F3EF),
    primary: dark ? const Color(0xFFDDEAE4) : const Color(0xFF23665B),
    secondary: dark ? const Color(0xFF98B7D5) : const Color(0xFF416B92),
    tertiary: dark ? const Color(0xFF85BDAE) : const Color(0xFF276958),
    onSurface: dark ? const Color(0xFFEDF0EE) : const Color(0xFF202623),
    onSurfaceVariant: dark ? const Color(0xFF9AA6A3) : const Color(0xFF62706A),
    onPrimary: dark ? const Color(0xFF172821) : Colors.white,
    surfaceContainerHighest: dark
        ? const Color(0xFF282E32)
        : const Color(0xFFE7ECE9),
    surfaceContainerLow: dark ? const Color(0xFF1A1E21) : Colors.white,
    outlineVariant: dark ? const Color(0xFF343B3F) : const Color(0xFFD7DFDA),
  );
  if (desktop) {
    scheme = scheme.copyWith(
      surface: dark ? const Color(0xFF191C1D) : const Color(0xFFFAF9F6),
      surfaceContainerLow: dark
          ? const Color(0xFF202425)
          : const Color(0xFFF0EFEB),
      surfaceContainerHighest: dark
          ? const Color(0xFF2D3333)
          : const Color(0xFFE7E9E4),
      primary: dark ? const Color(0xFFA1C4B8) : const Color(0xFF315F53),
      primaryContainer: dark
          ? const Color(0xFF30443E)
          : const Color(0xFFE0E8E0),
      onPrimary: dark ? const Color(0xFF172821) : const Color(0xFFFAF9F6),
      onPrimaryContainer: dark
          ? const Color(0xFFDCE9E1)
          : const Color(0xFF284C40),
      onSurface: dark ? const Color(0xFFE7EAE6) : const Color(0xFF292F2C),
      onSurfaceVariant: dark
          ? const Color(0xFFA7B0AA)
          : const Color(0xFF626D65),
      outlineVariant: dark ? const Color(0xFF39413E) : const Color(0xFFD8DDD5),
    );
  }
  final theme = ThemeData(
    useMaterial3: true,
    fontFamily: 'Inter',
    splashFactory: InkSparkle.splashFactory,
    pageTransitionsTheme: const PageTransitionsTheme(
      builders: {
        TargetPlatform.android: WorkspacePageTransitions(),
        TargetPlatform.iOS: WorkspacePageTransitions(),
        TargetPlatform.linux: WorkspacePageTransitions(),
        TargetPlatform.macOS: WorkspacePageTransitions(),
        TargetPlatform.windows: WorkspacePageTransitions(),
      },
    ),
    colorScheme: scheme,
    scaffoldBackgroundColor: scheme.surface,
    appBarTheme: AppBarTheme(
      backgroundColor: scheme.surface,
      surfaceTintColor: Colors.transparent,
      elevation: 0,
      centerTitle: false,
      titleTextStyle: TextStyle(
        fontFamily: 'Inter',
        fontSize: 18,
        fontWeight: FontWeight.w600,
        color: scheme.onSurface,
        letterSpacing: -0.4,
      ),
    ),
    dividerTheme: DividerThemeData(
      color: scheme.outlineVariant,
      thickness: 0.5,
      space: 1,
    ),
    navigationBarTheme: NavigationBarThemeData(
      height: 72,
      backgroundColor: scheme.surface,
      indicatorColor: scheme.primary.withValues(alpha: 0.12),
      elevation: 0,
    ),
    tabBarTheme: TabBarThemeData(
      dividerColor: scheme.outlineVariant,
      dividerHeight: 0.5,
      indicatorColor: scheme.primary,
      indicatorSize: TabBarIndicatorSize.label,
      labelColor: scheme.onSurface,
      unselectedLabelColor: scheme.onSurfaceVariant,
      labelStyle: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
      unselectedLabelStyle: const TextStyle(
        fontSize: 13,
        fontWeight: FontWeight.w500,
      ),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: scheme.surfaceContainerHighest.withValues(alpha: 0.4),
      hintStyle: TextStyle(
        color: scheme.onSurfaceVariant,
        fontSize: 14,
        fontWeight: FontWeight.w400,
      ),
      labelStyle: TextStyle(color: scheme.onSurfaceVariant, fontSize: 13),
      contentPadding: const EdgeInsets.symmetric(horizontal: 14, vertical: 14),
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(14),
        borderSide: BorderSide.none,
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(14),
        borderSide: BorderSide(color: scheme.tertiary, width: 1.5),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(14),
        borderSide: BorderSide.none,
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        minimumSize: const Size(48, 48),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      ),
    ),
    bottomSheetTheme: BottomSheetThemeData(
      backgroundColor: scheme.surface,
      modalBackgroundColor: scheme.surface,
      showDragHandle: true,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(28)),
      ),
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: scheme.surface,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(28)),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(minimumSize: const Size(48, 44)),
    ),
    textTheme: const TextTheme(
      headlineLarge: TextStyle(
        fontSize: 34,
        height: 1.1,
        fontWeight: FontWeight.w600,
        letterSpacing: -1.3,
      ),
      titleMedium: TextStyle(
        fontSize: 17,
        fontWeight: FontWeight.w600,
        letterSpacing: -0.4,
      ),
      labelLarge: TextStyle(
        fontSize: 12,
        fontWeight: FontWeight.w500,
        letterSpacing: 0.3,
      ),
      bodyMedium: TextStyle(fontSize: 15, height: 1.5),
      titleLarge: TextStyle(
        fontSize: 22,
        fontWeight: FontWeight.w600,
        letterSpacing: -0.4,
      ),
    ),
  );
  if (!desktop) return theme;
  final shape = RoundedRectangleBorder(borderRadius: BorderRadius.circular(8));
  return theme.copyWith(
    visualDensity: VisualDensity.compact,
    splashFactory: InkRipple.splashFactory,
    appBarTheme: theme.appBarTheme.copyWith(toolbarHeight: 56),
    iconTheme: theme.iconTheme.copyWith(size: 19),
    iconButtonTheme: IconButtonThemeData(
      style: IconButton.styleFrom(
        minimumSize: const Size(36, 36),
        padding: const EdgeInsets.all(8),
        shape: shape,
      ),
    ),
    listTileTheme: ListTileThemeData(
      dense: true,
      minVerticalPadding: 10,
      contentPadding: const EdgeInsets.symmetric(horizontal: 16),
      shape: shape,
      selectedTileColor: scheme.primaryContainer,
      selectedColor: scheme.onPrimaryContainer,
    ),
    inputDecorationTheme: theme.inputDecorationTheme.copyWith(
      fillColor: scheme.surfaceContainerLow,
      isDense: true,
      contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 12),
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: BorderSide.none,
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: BorderSide.none,
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: BorderSide(color: scheme.primary),
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        minimumSize: const Size(40, 38),
        shape: shape,
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        minimumSize: const Size(40, 36),
        shape: shape,
        side: BorderSide(color: scheme.outlineVariant),
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(
        minimumSize: const Size(36, 36),
        shape: shape,
      ),
    ),
    segmentedButtonTheme: SegmentedButtonThemeData(
      style: ButtonStyle(
        shape: WidgetStatePropertyAll(shape),
        side: WidgetStatePropertyAll(BorderSide(color: scheme.outlineVariant)),
        backgroundColor: WidgetStateProperty.resolveWith(
          (states) => states.contains(WidgetState.selected)
              ? scheme.primaryContainer
              : Colors.transparent,
        ),
        foregroundColor: WidgetStatePropertyAll(scheme.onSurface),
      ),
    ),
    textTheme: theme.textTheme
        .copyWith(
          headlineLarge: const TextStyle(
            fontFamily: 'Inter',
            fontSize: 28,
            height: 1.2,
            fontWeight: FontWeight.w600,
            letterSpacing: -.8,
          ),
          titleLarge: const TextStyle(
            fontFamily: 'Inter',
            fontSize: 20,
            fontWeight: FontWeight.w600,
            letterSpacing: -.5,
          ),
          titleMedium: const TextStyle(
            fontFamily: 'Inter',
            fontSize: 15,
            fontWeight: FontWeight.w600,
            letterSpacing: -.2,
          ),
          bodyMedium: const TextStyle(
            fontFamily: 'Inter',
            fontSize: 14,
            height: 1.5,
          ),
          labelLarge: const TextStyle(
            fontFamily: 'Inter',
            fontSize: 13,
            fontWeight: FontWeight.w500,
          ),
        )
        .apply(bodyColor: scheme.onSurface, displayColor: scheme.onSurface),
    dialogTheme: theme.dialogTheme.copyWith(
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
    ),
  );
}

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
    // Keep encrypted API keys available to Linux model discovery and bot
    // setup. Desktop initialization still skips loading/renewing OAuth.
    ModelCatalog.credentials = _credentials;
    _initialize();
    BackgroundRooms.channel.setMethodCallHandler((call) async {
      if (call.method == 'pauseAll') {
        for (final room in _library.sessions.values.toList()) {
          if (room.isRunning && room.store?.isPaused == false) {
            await room.togglePause();
          }
        }
      }
    });
  }

  Future<void> _initialize() async {
    // Ask once, early. A room that stops overnight should be able to say so
    // rather than be discovered in the morning.
    unawaited(RoomStore.attentions.start());
    try {
      // Loading the local store makes saved API keys available on Linux. This
      // is a keystore read only; desktop model discovery never renews OAuth.
      await _credentials.load();
      await _library.load();
      if (!canRunAgentClis) await _library.shareExistingKeys(_credentials);
    } catch (_) {
      _library.reportError(
        'An account could not be saved. Your existing keys were kept.',
      );
    }
  }

  @override
  void dispose() {
    BackgroundRooms.channel.setMethodCallHandler(null);
    _library.dispose();
    _credentials.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MaterialApp(
    title: 'OrgBots',
    debugShowCheckedModeBanner: false,
    theme: orgBotTheme(Brightness.light, desktop: canRunAgentClis),
    darkTheme: orgBotTheme(Brightness.dark, desktop: canRunAgentClis),
    themeMode: ThemeMode.system,
    home: OrgBotScreen(library: _library, credentials: _credentials),
  );
}
