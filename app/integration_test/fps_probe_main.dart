import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

/// The smallest possible animated app: how fast can this machine show frames?
Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const MaterialApp(home: _Spin()));
  await Future<void>.delayed(const Duration(seconds: 2));
  final frames = <ui.FrameTiming>[];
  void collect(List<ui.FrameTiming> timings) => frames.addAll(timings);
  WidgetsBinding.instance.addTimingsCallback(collect);
  final clock = Stopwatch()..start();
  await Future<void>.delayed(const Duration(seconds: 5));
  clock.stop();
  WidgetsBinding.instance.removeTimingsCallback(collect);
  final raster = frames.map((f) => f.rasterDuration.inMicroseconds).toList()..sort();
  final gaps = <int>[];
  for (var i = 1; i < frames.length; i++) {
    gaps.add(frames[i].timestampInMicroseconds(ui.FramePhase.vsyncStart) -
        frames[i - 1].timestampInMicroseconds(ui.FramePhase.vsyncStart));
  }
  gaps.sort();
  // ignore: avoid_print
  print('PROBE ${jsonEncode({
    'fps': (frames.length / (clock.elapsedMicroseconds / 1e6)).round(),
    'raster_p50_us': raster.isEmpty ? 0 : raster[raster.length ~/ 2],
    'vsync_gap_p50_us': gaps.isEmpty ? 0 : gaps[gaps.length ~/ 2],
    'vsync_gap_p90_us': gaps.isEmpty ? 0 : gaps[(gaps.length * .9).floor()],
  })}');
  exit(0);
}

class _Spin extends StatefulWidget {
  const _Spin();
  @override
  State<_Spin> createState() => _SpinState();
}

class _SpinState extends State<_Spin> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(
    vsync: this,
    duration: const Duration(seconds: 1),
  )..repeat();
  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    body: Center(
      child: RotationTransition(
        turns: _c,
        child: Container(width: 120, height: 120, color: Colors.teal),
      ),
    ),
  );
}
