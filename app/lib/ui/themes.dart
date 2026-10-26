// Appearance: the theme families, light/dark mode, and the picker that
// chooses between them. The choice is saved per user and applies to every
// window and room.
library;

import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// The colours one theme family uses in one brightness.
@immutable
class ThemePalette {
  const ThemePalette({
    required this.surface,
    required this.surfaceLow,
    required this.surfaceHigh,
    required this.primary,
    required this.onPrimary,
    required this.primaryContainer,
    required this.onPrimaryContainer,
    required this.secondary,
    required this.tertiary,
    required this.onSurface,
    required this.onSurfaceVariant,
    required this.outline,
    this.solid,
  });

  final Color surface, surfaceLow, surfaceHigh;
  final Color primary, onPrimary, primaryContainer, onPrimaryContainer;
  final Color secondary, tertiary;
  final Color onSurface, onSurfaceVariant, outline;

  /// An opaque surface for menus and dialogs when [surface] is translucent.
  final Color? solid;
}

/// A named theme with a light and a dark palette.
@immutable
class AppTheme {
  const AppTheme({
    required this.id,
    required this.name,
    required this.description,
    this.light,
    this.dark,
    this.glass = false,
  });

  final String id;
  final String name;
  final String description;

  /// Null for the original theme, which keeps its hand-tuned scheme.
  final ThemePalette? light, dark;

  /// Panels are translucent over a soft coloured backdrop.
  final bool glass;

  ThemePalette? palette(Brightness brightness) =>
      brightness == Brightness.dark ? dark : light;

  /// The colour behind translucent panels, for contrast checks and the
  /// window backdrop.
  Color backdropBase(Brightness brightness) => brightness == Brightness.dark
      ? const Color(0xFF0D121C)
      : const Color(0xFFE6ECF4);

  static const original = AppTheme(
    id: 'original',
    name: 'Original',
    description: 'Calm sage on soft neutrals',
  );

  static const paper = AppTheme(
    id: 'paper',
    name: 'Paper',
    description: 'Warm stock and ink blue',
    light: ThemePalette(
      surface: Color(0xFFF5F0E6),
      surfaceLow: Color(0xFFEDE6D8),
      surfaceHigh: Color(0xFFE2D9C7),
      primary: Color(0xFF2F4A6B),
      onPrimary: Color(0xFFFFFFFF),
      primaryContainer: Color(0xFFDCE2E8),
      onPrimaryContainer: Color(0xFF1D3048),
      secondary: Color(0xFF7A5C3A),
      tertiary: Color(0xFF4E6A49),
      onSurface: Color(0xFF2A2520),
      onSurfaceVariant: Color(0xFF675D51),
      outline: Color(0xFFD2C8B5),
    ),
    dark: ThemePalette(
      surface: Color(0xFF1D1A16),
      surfaceLow: Color(0xFF25211C),
      surfaceHigh: Color(0xFF342F28),
      primary: Color(0xFFD9C7A0),
      onPrimary: Color(0xFF2A2214),
      primaryContainer: Color(0xFF3F3727),
      onPrimaryContainer: Color(0xFFEFE2C4),
      secondary: Color(0xFFC8AA83),
      tertiary: Color(0xFFAAC0A0),
      onSurface: Color(0xFFEBE4D7),
      onSurfaceVariant: Color(0xFFB5AA96),
      outline: Color(0xFF443D33),
    ),
  );

  static const blackGold = AppTheme(
    id: 'black_gold',
    name: 'Black & Gold',
    description: 'Onyx with burnished gold',
    light: ThemePalette(
      surface: Color(0xFFFAF7F0),
      surfaceLow: Color(0xFFF2EDE1),
      surfaceHigh: Color(0xFFE7DFCD),
      primary: Color(0xFF7A5C0C),
      onPrimary: Color(0xFFFFFFFF),
      primaryContainer: Color(0xFFF1E3B8),
      onPrimaryContainer: Color(0xFF473705),
      secondary: Color(0xFF6E5A2E),
      tertiary: Color(0xFF7C6420),
      onSurface: Color(0xFF1C1A16),
      onSurfaceVariant: Color(0xFF5E5649),
      outline: Color(0xFFDCD1BC),
    ),
    dark: ThemePalette(
      surface: Color(0xFF0B0B0C),
      surfaceLow: Color(0xFF141414),
      surfaceHigh: Color(0xFF232120),
      primary: Color(0xFFD9B44A),
      onPrimary: Color(0xFF1A1406),
      primaryContainer: Color(0xFF3A3017),
      onPrimaryContainer: Color(0xFFF4DF9C),
      secondary: Color(0xFFCAA96B),
      tertiary: Color(0xFFE3C377),
      onSurface: Color(0xFFF3EFE7),
      onSurfaceVariant: Color(0xFFB5AD9D),
      outline: Color(0xFF35312A),
    ),
  );

  static const liquidGlass = AppTheme(
    id: 'liquid_glass',
    name: 'Liquid Glass',
    description: 'Frosted panels over soft light',
    glass: true,
    light: ThemePalette(
      surface: Color(0x9EFFFFFF),
      surfaceLow: Color(0x8CFFFFFF),
      surfaceHigh: Color(0xB8F3F6FA),
      primary: Color(0xFF1D5BB8),
      onPrimary: Color(0xFFFFFFFF),
      primaryContainer: Color(0xCCDCE7F8),
      onPrimaryContainer: Color(0xFF123E80),
      secondary: Color(0xFF3F6E8C),
      tertiary: Color(0xFF2E7A6A),
      onSurface: Color(0xFF151C27),
      onSurfaceVariant: Color(0xFF4C5869),
      outline: Color(0x2E0B1B33),
      solid: Color(0xFFF6F8FB),
    ),
    dark: ThemePalette(
      surface: Color(0x8C131A27),
      surfaceLow: Color(0x80192232),
      surfaceHigh: Color(0x99273246),
      primary: Color(0xFF93C4FF),
      onPrimary: Color(0xFF0B2340),
      primaryContainer: Color(0xB3203A5E),
      onPrimaryContainer: Color(0xFFD8E8FF),
      secondary: Color(0xFFA6C2DA),
      tertiary: Color(0xFF8FD1BF),
      onSurface: Color(0xFFEFF3F9),
      onSurfaceVariant: Color(0xFFAEB9CB),
      outline: Color(0x29FFFFFF),
      solid: Color(0xFF161D2A),
    ),
  );

  static const midnight = AppTheme(
    id: 'midnight',
    name: 'Midnight',
    description: 'Deep navy and ice blue',
    light: ThemePalette(
      surface: Color(0xFFF4F6FB),
      surfaceLow: Color(0xFFECEFF6),
      surfaceHigh: Color(0xFFDFE4EE),
      primary: Color(0xFF2B4C9B),
      onPrimary: Color(0xFFFFFFFF),
      primaryContainer: Color(0xFFDCE4F8),
      onPrimaryContainer: Color(0xFF17306B),
      secondary: Color(0xFF4A6491),
      tertiary: Color(0xFF2F7186),
      onSurface: Color(0xFF1A1F2B),
      onSurfaceVariant: Color(0xFF5A6375),
      outline: Color(0xFFD4DAE6),
    ),
    dark: ThemePalette(
      surface: Color(0xFF0F1420),
      surfaceLow: Color(0xFF161D2C),
      surfaceHigh: Color(0xFF222B3D),
      primary: Color(0xFF9DB8FF),
      onPrimary: Color(0xFF0E1A3A),
      primaryContainer: Color(0xFF26355C),
      onPrimaryContainer: Color(0xFFD9E2FF),
      secondary: Color(0xFFA9BCE0),
      tertiary: Color(0xFF8CCFE0),
      onSurface: Color(0xFFE6EAF2),
      onSurfaceVariant: Color(0xFFA3ACBD),
      outline: Color(0xFF2C3446),
    ),
  );

  static const graphite = AppTheme(
    id: 'graphite',
    name: 'Graphite',
    description: 'Quiet monochrome',
    light: ThemePalette(
      surface: Color(0xFFF7F7F6),
      surfaceLow: Color(0xFFEFEFEE),
      surfaceHigh: Color(0xFFE3E3E1),
      primary: Color(0xFF262626),
      onPrimary: Color(0xFFFFFFFF),
      primaryContainer: Color(0xFFE2E2E2),
      onPrimaryContainer: Color(0xFF1F1F1F),
      secondary: Color(0xFF4D4D4D),
      tertiary: Color(0xFF3D5A50),
      onSurface: Color(0xFF1C1C1C),
      onSurfaceVariant: Color(0xFF5E5E5E),
      outline: Color(0xFFD9D9D7),
    ),
    dark: ThemePalette(
      surface: Color(0xFF151515),
      surfaceLow: Color(0xFF1C1C1C),
      surfaceHigh: Color(0xFF2A2A2A),
      primary: Color(0xFFE2E2E2),
      onPrimary: Color(0xFF171717),
      primaryContainer: Color(0xFF333333),
      onPrimaryContainer: Color(0xFFEDEDED),
      secondary: Color(0xFFBDBDBD),
      tertiary: Color(0xFF9CC2B3),
      onSurface: Color(0xFFECECEC),
      onSurfaceVariant: Color(0xFFA6A6A6),
      outline: Color(0xFF333333),
    ),
  );

  static const rosewood = AppTheme(
    id: 'rosewood',
    name: 'Rosewood',
    description: 'Dusty rose and dark wine',
    light: ThemePalette(
      surface: Color(0xFFFBF6F5),
      surfaceLow: Color(0xFFF4ECEA),
      surfaceHigh: Color(0xFFE9DDDA),
      primary: Color(0xFF8C3B4E),
      onPrimary: Color(0xFFFFFFFF),
      primaryContainer: Color(0xFFF5DCE2),
      onPrimaryContainer: Color(0xFF5A1E2D),
      secondary: Color(0xFF7A5560),
      tertiary: Color(0xFF6B6A3E),
      onSurface: Color(0xFF2A1E21),
      onSurfaceVariant: Color(0xFF6E5A5F),
      outline: Color(0xFFE0D2D0),
    ),
    dark: ThemePalette(
      surface: Color(0xFF1A1416),
      surfaceLow: Color(0xFF221A1D),
      surfaceHigh: Color(0xFF32272B),
      primary: Color(0xFFE8A7B5),
      onPrimary: Color(0xFF3A1520),
      primaryContainer: Color(0xFF4A2A33),
      onPrimaryContainer: Color(0xFFF7D9E0),
      secondary: Color(0xFFD3B0B9),
      tertiary: Color(0xFFCBC991),
      onSurface: Color(0xFFF1E7E9),
      onSurfaceVariant: Color(0xFFBAA7AC),
      outline: Color(0xFF3D3034),
    ),
  );

  static const all = [
    original,
    paper,
    blackGold,
    liquidGlass,
    midnight,
    graphite,
    rosewood,
  ];

  static AppTheme byId(String? id) =>
      all.firstWhere((theme) => theme.id == id, orElse: () => original);

  /// A full colour scheme from a palette, filling the roles the palette
  /// leaves out from a matching seed.
  static ColorScheme scheme(ThemePalette p, Brightness brightness) {
    final seeded = ColorScheme.fromSeed(
      seedColor: p.primary,
      brightness: brightness,
    );
    final container = p.solid ?? Color.lerp(p.surfaceLow, p.surfaceHigh, .5)!;
    return seeded.copyWith(
      surface: p.surface,
      surfaceContainerLowest: p.solid ?? p.surface,
      surfaceContainerLow: p.surfaceLow,
      surfaceContainer: container,
      surfaceContainerHigh: p.solid ?? Color.lerp(p.surfaceLow, p.surfaceHigh, .75),
      surfaceContainerHighest: p.surfaceHigh,
      primary: p.primary,
      onPrimary: p.onPrimary,
      primaryContainer: p.primaryContainer,
      onPrimaryContainer: p.onPrimaryContainer,
      secondary: p.secondary,
      tertiary: p.tertiary,
      onSurface: p.onSurface,
      onSurfaceVariant: p.onSurfaceVariant,
      outlineVariant: p.outline,
      surfaceTint: Colors.transparent,
    );
  }
}

/// The saved appearance: which theme, and light, dark or the system's.
class Appearance extends ChangeNotifier {
  Appearance._();

  @visibleForTesting
  Appearance.forTesting();

  static final Appearance instance = Appearance._();

  static const _themeKey = 'appearance.theme';
  static const _modeKey = 'appearance.mode';

  AppTheme _theme = AppTheme.original;
  ThemeMode _mode = ThemeMode.system;

  AppTheme get theme => _theme;
  ThemeMode get mode => _mode;

  Future<void> load() async {
    try {
      final prefs = await SharedPreferences.getInstance();
      final theme = AppTheme.byId(prefs.getString(_themeKey));
      final mode = ThemeMode.values
          .where((m) => m.name == prefs.getString(_modeKey))
          .firstOrNull;
      if (theme != _theme || (mode != null && mode != _mode)) {
        _theme = theme;
        _mode = mode ?? _mode;
        notifyListeners();
      }
    } catch (_) {
      // A missing preference store keeps the defaults.
    }
  }

  Future<void> setTheme(AppTheme theme) async {
    if (theme == _theme) return;
    _theme = theme;
    notifyListeners();
    await _save(_themeKey, theme.id);
  }

  Future<void> setMode(ThemeMode mode) async {
    if (mode == _mode) return;
    _mode = mode;
    notifyListeners();
    await _save(_modeKey, mode.name);
  }

  Future<void> _save(String key, String value) async {
    try {
      final prefs = await SharedPreferences.getInstance();
      await prefs.setString(key, value);
    } catch (_) {
      // The choice still applies for this session.
    }
  }
}

/// The soft, pre-blurred light that glass panels sit over.
///
/// Painted once and cached: panels are translucent over an already blurred
/// image, so they read as frosted without a per-frame backdrop filter.
class GlassBackdrop extends StatelessWidget {
  const GlassBackdrop({super.key});

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    final base = AppTheme.liquidGlass.backdropBase(
      dark ? Brightness.dark : Brightness.light,
    );
    final blobs = dark
        ? const [
            (Alignment(-0.9, -0.8), Color(0xFF1F4E8C), 0.55),
            (Alignment(0.95, -0.35), Color(0xFF1C6B66), 0.45),
            (Alignment(0.2, 0.95), Color(0xFF6B4A1F), 0.40),
          ]
        : const [
            (Alignment(-0.9, -0.8), Color(0xFFA9C8F2), 0.55),
            (Alignment(0.95, -0.35), Color(0xFFB4E3D6), 0.50),
            (Alignment(0.2, 0.95), Color(0xFFF4D6B8), 0.50),
          ];
    return RepaintBoundary(
          child: ColoredBox(
            color: base,
            child: ImageFiltered(
              imageFilter: ui.ImageFilter.blur(sigmaX: 90, sigmaY: 90),
              child: Stack(
                fit: StackFit.expand,
                children: [
                  for (final (alignment, color, size) in blobs)
                    Align(
                      alignment: alignment,
                      child: FractionallySizedBox(
                        widthFactor: size,
                        heightFactor: size,
                        child: DecoratedBox(
                          decoration: BoxDecoration(
                            color: color,
                            shape: BoxShape.circle,
                          ),
                        ),
                      ),
                    ),
                ],
              ),
            ),
          ),
        );
  }
}

/// The Appearance section: mode and theme family.
class AppearancePicker extends StatelessWidget {
  const AppearancePicker({super.key, this.appearance});

  /// Defaults to the app-wide [Appearance.instance].
  final Appearance? appearance;

  @override
  Widget build(BuildContext context) {
    final settings = appearance ?? Appearance.instance;
    return ListenableBuilder(
      listenable: settings,
      builder: (context, _) {
        final brightness = Theme.of(context).brightness;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Wrap(
              alignment: WrapAlignment.spaceBetween,
              crossAxisAlignment: WrapCrossAlignment.center,
              spacing: 16,
              runSpacing: 10,
              children: [
                Text(
                  'Appearance',
                  style: Theme.of(context).textTheme.titleMedium,
                ),
                SegmentedButton<ThemeMode>(
                  key: const ValueKey('appearance-mode'),
                  showSelectedIcon: false,
                  segments: const [
                    ButtonSegment(
                      value: ThemeMode.system,
                      label: Text('System'),
                      icon: Icon(Icons.brightness_auto_outlined, size: 16),
                    ),
                    ButtonSegment(
                      value: ThemeMode.light,
                      label: Text('Light'),
                      icon: Icon(Icons.light_mode_outlined, size: 16),
                    ),
                    ButtonSegment(
                      value: ThemeMode.dark,
                      label: Text('Dark'),
                      icon: Icon(Icons.dark_mode_outlined, size: 16),
                    ),
                  ],
                  selected: {settings.mode},
                  onSelectionChanged: (value) => settings.setMode(value.first),
                ),
              ],
            ),
            const SizedBox(height: 12),
            Wrap(
              spacing: 12,
              runSpacing: 12,
              children: [
                for (final theme in AppTheme.all)
                  _ThemeCard(
                    key: ValueKey('theme-${theme.id}'),
                    theme: theme,
                    brightness: brightness,
                    selected: theme == settings.theme,
                    onTap: () => settings.setTheme(theme),
                  ),
              ],
            ),
            const SizedBox(height: 20),
          ],
        );
      },
    );
  }
}

class _ThemeCard extends StatelessWidget {
  const _ThemeCard({
    super.key,
    required this.theme,
    required this.brightness,
    required this.selected,
    required this.onTap,
  });

  final AppTheme theme;
  final Brightness brightness;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final p = theme.palette(brightness);
    final dark = brightness == Brightness.dark;
    // The original theme has no palette object; its preview uses its colours.
    final surface = p?.surface ?? (dark ? const Color(0xFF191C1D) : const Color(0xFFFAF9F6));
    final side = p?.surfaceLow ?? (dark ? const Color(0xFF202425) : const Color(0xFFF0EFEB));
    final primary = p?.primary ?? (dark ? const Color(0xFFA1C4B8) : const Color(0xFF315F53));
    final ink = p?.onSurface ?? (dark ? const Color(0xFFE7EAE6) : const Color(0xFF292F2C));
    final muted = p?.onSurfaceVariant ?? (dark ? const Color(0xFFA7B0AA) : const Color(0xFF5A645D));
    final base = theme.glass ? theme.backdropBase(brightness) : surface;
    Widget line(double width, Color color) => Container(
      width: width,
      height: 5,
      decoration: BoxDecoration(
        color: color,
        borderRadius: BorderRadius.circular(3),
      ),
    );
    return Semantics(
      button: true,
      selected: selected,
      label: '${theme.name} theme',
      child: Material(
        color: Colors.transparent,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(10),
          side: BorderSide(
            color: selected ? scheme.primary : scheme.outlineVariant,
            width: selected ? 2 : 1,
          ),
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: SizedBox(
            width: 176,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                SizedBox(
                  height: 84,
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      color: base,
                      gradient: theme.glass
                          ? LinearGradient(
                              begin: Alignment.topLeft,
                              end: Alignment.bottomRight,
                              colors: dark
                                  ? const [Color(0xFF1F4E8C), Color(0xFF0D121C), Color(0xFF1C6B66)]
                                  : const [Color(0xFFA9C8F2), Color(0xFFE6ECF4), Color(0xFFF4D6B8)],
                            )
                          : null,
                    ),
                    child: Row(
                      children: [
                        Container(
                          width: 38,
                          margin: theme.glass ? const EdgeInsets.all(6) : null,
                          decoration: BoxDecoration(
                            color: side,
                            borderRadius: theme.glass
                                ? BorderRadius.circular(6)
                                : null,
                          ),
                        ),
                        Expanded(
                          child: Container(
                            margin: theme.glass
                                ? const EdgeInsets.fromLTRB(0, 6, 6, 6)
                                : null,
                            padding: const EdgeInsets.all(10),
                            decoration: BoxDecoration(
                              color: theme.glass ? surface : null,
                              borderRadius: theme.glass
                                  ? BorderRadius.circular(6)
                                  : null,
                            ),
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                line(46, ink),
                                const SizedBox(height: 6),
                                line(80, muted.withValues(alpha: .6)),
                                const SizedBox(height: 4),
                                line(64, muted.withValues(alpha: .6)),
                                const Spacer(),
                                Container(
                                  width: 34,
                                  height: 12,
                                  decoration: BoxDecoration(
                                    color: primary,
                                    borderRadius: BorderRadius.circular(6),
                                  ),
                                ),
                              ],
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
                Padding(
                  padding: const EdgeInsets.fromLTRB(10, 8, 8, 9),
                  child: Row(
                    children: [
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              theme.name,
                              style: const TextStyle(
                                fontSize: 13,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                            Text(
                              theme.description,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontSize: 11,
                                color: scheme.onSurfaceVariant,
                              ),
                            ),
                          ],
                        ),
                      ),
                      AnimatedOpacity(
                        opacity: selected ? 1 : 0,
                        duration: const Duration(milliseconds: 150),
                        child: Icon(
                          Icons.check_circle,
                          size: 18,
                          color: scheme.primary,
                        ),
                      ),
                    ],
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
