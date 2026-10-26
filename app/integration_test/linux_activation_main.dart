// Minimal Flutter entrypoint for real GTK cold/warm activation acceptance.
// It uses the production activation MethodChannel but no room or provider APIs.

import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:orgbot_app/orgbot/attention.dart';

void main(List<String> arguments) {
  WidgetsFlutterBinding.ensureInitialized();
  final capture = Platform.environment['ORGBOTS_ACTIVATION_CAPTURE_FILE'];
  if (capture == null || capture.isEmpty) {
    stderr.writeln('ORGBOTS_ACTIVATION_CAPTURE_FILE is required');
    exit(64);
  }
  File('$capture.args.json').writeAsStringSync(jsonEncode(arguments), flush: true);

  final attentions = Attentions();
  attentions.listenForDesktopActivation(arguments);
  attentions.onRouteRequested = (target) {
    File(capture).writeAsStringSync(
      '${jsonEncode(target.toJson())}\n',
      mode: FileMode.append,
      flush: true,
    );
  };

  runApp(const MaterialApp(home: Scaffold(body: Center(child: Text('Ready')))));
}
