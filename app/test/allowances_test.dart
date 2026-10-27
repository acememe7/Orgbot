import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/orgbot/allowances.dart';
import 'package:orgbot_app/ui/allowances_view.dart';

class MemoryAllowances extends DesktopAllowances {
  MemoryAllowances() : super(Directory.systemTemp);
  final values = <String, ProviderAllowance>{};
  @override
  Future<Map<String, ProviderAllowance>> policies() async => values;
  @override
  Future<Map<String, dynamic>> usage() async => {};
  @override
  Future<void> save(String provider, ProviderAllowance policy) async {
    policy.validate();
    values[provider] = policy;
  }
}

void main() {
  test(
    'model rates preserve explicit zero cache prices and legacy omissions',
    () {
      const configured = ProviderAllowance(
        inputPrice: 3,
        outputPrice: 7,
        cacheReadPrice: 0,
        cacheWritePrice: 0,
        modelRates: {'model-a': ModelTokenRates(11, 22, 0, 0)},
      );
      final restored = ProviderAllowance.fromJson(configured.toJson());
      expect(restored.cacheReadPrice, 0);
      expect(restored.cacheWritePrice, 0);
      expect(restored.modelRates['model-a']!.values, [11, 22, 0, 0]);

      final legacy = ProviderAllowance.fromJson({
        'input_microusd_per_million': 3,
        'output_microusd_per_million': 7,
      });
      expect(legacy.cacheReadPrice, isNull);
      expect(legacy.cacheWritePrice, isNull);
      expect(legacy.modelRates, isEmpty);
      expect(
        legacy.toJson().containsKey('cache_read_microusd_per_million'),
        isFalse,
      );
    },
  );

  test(
    'saved caps preserve other providers and reject invalid money settings',
    () async {
      final root = await Directory.systemTemp.createTemp('orgbot-allowances-');
      addTearDown(() => root.delete(recursive: true));
      final manager = DesktopAllowances(Directory('${root.path}/controls'));
      await manager.save('cli:codex', const ProviderAllowance(requests: 5));
      await manager.save('deepseek', const ProviderAllowance(tokens: 20000));
      final restored = await DesktopAllowances(manager.directory).policies();
      expect(restored['cli:codex']!.requests, 5);
      expect(restored['deepseek']!.tokens, 20000);
      await expectLater(
        manager.save(
          'deepseek',
          const ProviderAllowance(costMicrousd: 1000000),
        ),
        throwsFormatException,
      );
      expect((await manager.policies())['deepseek']!.costMicrousd, 0);
    },
  );

  test(
    'corrupt and linked settings fail instead of appearing unlimited',
    () async {
      final root = await Directory.systemTemp.createTemp('orgbot-allowances-');
      addTearDown(() => root.delete(recursive: true));
      final manager = DesktopAllowances(root);
      final file = File('${root.path}/policies.json');
      await file.writeAsString('invalid');
      await expectLater(manager.policies(), throwsFormatException);
      await file.delete();
      final target = File('${root.path}/other.json');
      await target.writeAsString('{}');
      await Link(file.path).create(target.path);
      await expectLater(manager.policies(), throwsFormatException);
    },
  );

  testWidgets('CLI controls save episode caps without promising token caps', (
    tester,
  ) async {
    final manager = MemoryAllowances();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(child: AllowancesView(manager: manager)),
        ),
      ),
    );
    await tester.tap(find.text('Spending controls'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Codex CLI'));
    await tester.pumpAndSettle();
    expect(find.text('Tokens (0 = unlimited)'), findsNothing);
    expect(
      find.text(
        'CLI caps count episodes only. Charges depend on the selected CLI provider and account. API-equivalent prices are estimates; OrgBots does not enforce CLI dollar or token caps.',
      ),
      findsOneWidget,
    );
    await tester.enterText(find.byType(TextFormField).first, '12');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(manager.values['cli:codex']!.requests, 12);
    expect(find.text('12 requests per week'), findsOneWidget);
    expect(find.text('OpenCode CLI'), findsOneWidget);
    expect(find.text('Antigravity CLI'), findsOneWidget);
    await tester.tap(find.text('Antigravity CLI'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextFormField).first, '8');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(manager.values['cli:agy']!.requests, 8);
  });
}
