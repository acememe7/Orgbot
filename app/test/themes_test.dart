import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/main.dart' show orgBotTheme;
import 'package:orgbot_app/ui/themes.dart';
import 'package:shared_preferences/shared_preferences.dart';

double _contrast(Color a, Color b) {
  final la = a.computeLuminance(), lb = b.computeLuminance();
  return la > lb ? (la + .05) / (lb + .05) : (lb + .05) / (la + .05);
}

/// What a translucent panel actually looks like over the glass backdrop.
Color _over(Color color, Color base) =>
    color.a >= 1 ? color : Color.alphaBlend(color, base);

void main() {
  for (final appTheme in AppTheme.all) {
    for (final brightness in Brightness.values) {
      test('${appTheme.name} ${brightness.name} keeps text readable', () {
        final theme = orgBotTheme(
          brightness,
          desktop: true,
          appTheme: appTheme,
        );
        final scheme = theme.colorScheme;
        final base = appTheme.backdropBase(brightness);
        for (final surface in [
          scheme.surface,
          scheme.surfaceContainerLow,
          scheme.surfaceContainerHighest,
        ]) {
          final shown = _over(surface, base);
          expect(
            _contrast(scheme.onSurface, shown),
            greaterThanOrEqualTo(7),
            reason: 'body text on $surface',
          );
          expect(
            _contrast(scheme.onSurfaceVariant, shown),
            greaterThanOrEqualTo(4.5),
            reason: 'secondary text on $surface',
          );
        }
        expect(
          _contrast(scheme.onPrimary, scheme.primary),
          greaterThanOrEqualTo(4.5),
          reason: 'filled buttons',
        );
        expect(
          _contrast(scheme.primary, _over(scheme.surface, base)),
          greaterThanOrEqualTo(4.5),
          reason: 'links and @mentions',
        );
        expect(
          _contrast(
            scheme.onPrimaryContainer,
            _over(scheme.primaryContainer, base),
          ),
          greaterThanOrEqualTo(4.5),
          reason: 'selected rows',
        );
        // Menus and dialogs are never see-through.
        expect(theme.popupMenuTheme.color!.a, 1);
        expect(theme.dialogTheme.backgroundColor!.a, 1);
      });
    }
  }

  test('an unknown saved theme falls back to the original', () {
    expect(AppTheme.byId('retired'), AppTheme.original);
    expect(AppTheme.byId('black_gold'), AppTheme.blackGold);
  });

  testWidgets('choosing a theme and mode applies and saves them', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final appearance = Appearance.forTesting();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: AppearancePicker(appearance: appearance),
          ),
        ),
      ),
    );
    expect(find.text('Paper'), findsOneWidget);
    expect(find.text('Black & Gold'), findsOneWidget);
    expect(find.text('Liquid Glass'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('theme-black_gold')));
    await tester.tap(find.text('Dark'));
    await tester.pumpAndSettle();
    expect(appearance.theme, AppTheme.blackGold);
    expect(appearance.mode, ThemeMode.dark);

    final restored = Appearance.forTesting();
    await restored.load();
    expect(restored.theme, AppTheme.blackGold);
    expect(restored.mode, ThemeMode.dark);
  });
}
