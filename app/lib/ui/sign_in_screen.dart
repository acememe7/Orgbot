// Signing in on a device that has no agent CLIs.
//
// The desktop signs in by running `claude`, `codex` or `grok`. A phone cannot,
// so it carries those sign-ins across instead: `orgbot pair` on the desktop
// prints a bundle, and this imports it. The tokens are the same OAuth tokens
// the CLIs hold, and the provider APIs accept them directly.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../orgbot/credentials.dart';
import 'brand.dart';

/// Import and manage this device's sign-ins.
class SignInScreen extends StatefulWidget {
  const SignInScreen({super.key, required this.credentials});

  final Credentials credentials;

  @override
  State<SignInScreen> createState() => _SignInScreenState();
}

class _SignInScreenState extends State<SignInScreen> {
  final TextEditingController _bundle = TextEditingController();
  String? _error;
  String? _success;
  bool _busy = false;

  @override
  void dispose() {
    _bundle.dispose();
    super.dispose();
  }

  Future<void> _import() async {
    setState(() {
      _busy = true;
      _error = null;
      _success = null;
    });
    try {
      final count = await widget.credentials.importBundle(_bundle.text);
      if (!mounted) return;
      _bundle.clear();
      setState(() {
        _success = count == 1
            ? 'Signed in to 1 provider.'
            : 'Signed in to $count providers.';
      });
    } on PairingError catch (e) {
      if (mounted) setState(() => _error = e.message);
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _paste() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text;
    if (text != null && text.trim().isNotEmpty) {
      _bundle.text = text.trim();
      await _import();
    }
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Scaffold(
      appBar: AppBar(
        titleSpacing: 12,
        title: Row(
          children: [
            const BrandMark(size: 26),
            const SizedBox(width: 10),
            const Text('Sign in', style: TextStyle(fontSize: 17)),
          ],
        ),
      ),
      body: ListenableBuilder(
        listenable: widget.credentials,
        builder: (context, _) {
          final signedIn = widget.credentials.all;
          return ListView(
            padding: const EdgeInsets.all(20),
            children: [
              if (signedIn.isNotEmpty) ...[
                Text('Signed in', style: Theme.of(context).textTheme.labelLarge),
                const SizedBox(height: 10),
                for (final credential in signedIn)
                  _CredentialCard(
                    credential: credential,
                    onForget: () =>
                        widget.credentials.forget(credential.provider),
                  ),
                const SizedBox(height: 24),
              ],
              Text(
                signedIn.isEmpty ? 'Bring your sign-ins here' : 'Add more',
                style: Theme.of(context).textTheme.labelLarge,
              ),
              const SizedBox(height: 8),
              Text(
                'This device has no agent CLIs to sign in with, so it uses the '
                'sign-ins from a machine that does. On that machine run:',
                style: TextStyle(
                    fontSize: 13, height: 1.4, color: scheme.onSurfaceVariant),
              ),
              const SizedBox(height: 10),
              Container(
                width: double.infinity,
                padding: const EdgeInsets.all(13),
                decoration: BoxDecoration(
                  color: scheme.surfaceContainerHighest,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: const SelectableText(
                  'orgbot pair',
                  style: TextStyle(fontFamily: 'monospace', fontSize: 13),
                ),
              ),
              const SizedBox(height: 10),
              Text(
                'Then paste everything it printed below. It contains live '
                'access tokens, so treat it like a password.',
                style: TextStyle(
                    fontSize: 12, height: 1.4, color: scheme.onSurfaceVariant),
              ),
              const SizedBox(height: 16),
              TextField(
                controller: _bundle,
                maxLines: 5,
                minLines: 3,
                style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
                decoration: const InputDecoration(
                  labelText: 'Pairing bundle',
                  hintText: '{"version":1,...}',
                  border: OutlineInputBorder(),
                  alignLabelWithHint: true,
                ),
              ),
              if (_error != null) ...[
                const SizedBox(height: 12),
                _Banner(text: _error!, tone: scheme.error, icon: Icons.error_outline),
              ],
              if (_success != null) ...[
                const SizedBox(height: 12),
                _Banner(
                    text: _success!,
                    tone: Colors.green,
                    icon: Icons.check_circle_outline),
              ],
              const SizedBox(height: 16),
              Row(
                children: [
                  Expanded(
                    child: FilledButton.icon(
                      onPressed: _busy ? null : _import,
                      icon: _busy
                          ? const SizedBox(
                              width: 15,
                              height: 15,
                              child: CircularProgressIndicator(strokeWidth: 2))
                          : const Icon(Icons.login),
                      label: const Text('Sign in'),
                    ),
                  ),
                  const SizedBox(width: 10),
                  OutlinedButton.icon(
                    onPressed: _busy ? null : _paste,
                    icon: const Icon(Icons.content_paste),
                    label: const Text('Paste'),
                  ),
                ],
              ),
              if (signedIn.isNotEmpty) ...[
                const SizedBox(height: 24),
                TextButton.icon(
                  onPressed: widget.credentials.forgetAll,
                  icon: const Icon(Icons.logout, size: 18),
                  label: const Text('Sign out of everything'),
                  style: TextButton.styleFrom(foregroundColor: scheme.error),
                ),
              ],
            ],
          );
        },
      ),
    );
  }
}

class _CredentialCard extends StatelessWidget {
  const _CredentialCard({required this.credential, required this.onForget});

  final ProviderCredential credential;
  final VoidCallback onForget;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      margin: const EdgeInsets.only(bottom: 9),
      padding: const EdgeInsets.all(13),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: scheme.outlineVariant),
      ),
      child: Row(
        children: [
          Icon(Icons.verified_user_outlined, size: 20, color: Colors.green),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(credential.provider.label,
                    style: const TextStyle(
                        fontWeight: FontWeight.w700, fontSize: 14)),
                const SizedBox(height: 2),
                Text(
                  [
                    if (credential.plan != null) credential.plan!,
                    if (credential.account != null) credential.account!,
                    '${credential.models.length} model(s)',
                  ].join(' · '),
                  style:
                      TextStyle(fontSize: 11.5, color: scheme.onSurfaceVariant),
                ),
              ],
            ),
          ),
          IconButton(
            tooltip: 'Sign out',
            icon: const Icon(Icons.close, size: 18),
            onPressed: onForget,
          ),
        ],
      ),
    );
  }
}

class _Banner extends StatelessWidget {
  const _Banner({required this.text, required this.tone, required this.icon});

  final String text;
  final Color tone;
  final IconData icon;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: tone.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: tone.withValues(alpha: 0.35)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 16, color: tone),
          const SizedBox(width: 9),
          Expanded(
            child: Text(text,
                style: TextStyle(fontSize: 12.5, height: 1.35, color: tone)),
          ),
        ],
      ),
    );
  }
}
