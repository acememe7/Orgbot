// Signing this device in to the subscriptions it will run bots on.
//
// Codex and Grok each publish a device-code flow that accepts their own CLI's
// public client id, so this device signs in by itself: it shows a short code,
// the person approves it in a browser, and the provider hands back a token.
// Nothing here depends on a desktop.
//
// Anthropic publishes no device flow, so a Claude subscription cannot be
// signed in to this way. On a device with the CLIs installed it is used
// directly; elsewhere it is absent rather than pretending to offer a sign-in
// that cannot work.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:url_launcher/url_launcher.dart';

import '../orgbot/credentials.dart';
import '../orgbot/models.dart';
import '../orgbot/oauth/device_auth.dart';
import 'brand.dart';
import 'polish.dart';

/// Sign in to providers, and manage what this device already holds.
class SignInScreen extends StatefulWidget {
  const SignInScreen({
    super.key,
    required this.credentials,
    this.auth,
    this.embedded = false,
  });

  final Credentials credentials;

  /// Injectable so the flows can be driven in tests without a network.
  final DeviceAuth? auth;
  final bool embedded;

  @override
  State<SignInScreen> createState() => _SignInScreenState();
}

class _SignInScreenState extends State<SignInScreen> {
  final TextEditingController _bundle = TextEditingController();
  String? _error;
  String? _success;
  bool _busy = false;
  bool _showPairing = false;

  /// The sign-in currently waiting on the person, if any.
  DeviceSignIn? _pending;

  DeviceAuth get _auth => widget.auth ?? DeviceAuth();

  Future<void> _apiKey(ProviderKind provider) async {
    final key = TextEditingController();
    String? error;
    var saving = false;
    final route = DialogRoute<void>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, update) => AlertDialog(
          title: Text('${provider.label} API key'),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: key,
                obscureText: true,
                autocorrect: false,
                enableSuggestions: false,
                decoration: InputDecoration(
                  labelText: 'API key',
                  errorText: error,
                  helperText:
                      'Saved securely and shared by your ${provider.label} bots.',
                ),
              ),
              if (provider == ProviderKind.ollama)
                TextButton.icon(
                  onPressed: () => launchUrl(
                    Uri.parse('https://ollama.com/settings/keys'),
                    mode: LaunchMode.externalApplication,
                  ),
                  icon: const Icon(Icons.open_in_new, size: 16),
                  label: const Text('Get an Ollama API key'),
                ),
            ],
          ),
          actions: [
            TextButton(
              onPressed: saving ? null : () => Navigator.pop(context),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: saving
                  ? null
                  : () async {
                      if (key.text.trim().isEmpty) {
                        update(() => error = 'Enter your API key');
                        return;
                      }
                      update(() {
                        saving = true;
                        error = null;
                      });
                      try {
                        await widget.credentials.saveApiKey(provider, key.text);
                        if (context.mounted) Navigator.pop(context);
                      } catch (_) {
                        if (context.mounted) {
                          update(() {
                            saving = false;
                            error = 'Could not save the key. Try again.';
                          });
                        }
                      }
                    },
              child: Text(saving ? 'Saving…' : 'Save key'),
            ),
          ],
        ),
      ),
    );
    await Navigator.push(context, route);
    await route.completed;
    key.dispose();
  }

  @override
  void dispose() {
    _pending?.cancel();
    _bundle.dispose();
    super.dispose();
  }

  void _report({String? error, String? success}) {
    if (!mounted) return;
    setState(() {
      _error = error;
      _success = success;
    });
  }

  Future<void> _signIn(ProviderKind provider) async {
    setState(() {
      _busy = true;
      _error = null;
      _success = null;
    });
    DeviceSignIn? flow;
    try {
      flow = await _auth.start(provider);
      if (!mounted) {
        flow.cancel();
        return;
      }
      setState(() {
        _pending = flow;
        _busy = false;
      });
      final token = await flow.token;
      await widget.credentials.signIn(provider, token);
      if (!mounted) return;
      setState(() => _pending = null);
      _report(success: 'Signed in to ' + provider.label + '.');
    } on SignInCancelled {
      if (mounted) setState(() => _pending = null);
    } on SignInError catch (e) {
      if (mounted) setState(() => _pending = null);
      _report(error: e.message);
    } catch (e) {
      if (mounted) setState(() => _pending = null);
      _report(error: e.toString());
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _cancelPending() {
    _pending?.cancel();
    setState(() => _pending = null);
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
      _report(
        success: count == 1
            ? 'Signed in to 1 provider.'
            : 'Signed in to $count providers.',
      );
    } on PairingError catch (e) {
      _report(error: e.message);
    } catch (e) {
      _report(error: e.toString());
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
      appBar: widget.embedded
          ? null
          : AppBar(
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
          final pending = _pending;
          if (pending != null) {
            return _DeviceCodePanel(flow: pending, onCancel: _cancelPending);
          }
          final signedIn = widget.credentials.all
              .where((credential) => !credential.provider.needsApiKey)
              .toList();
          return ListView(
            padding: const EdgeInsets.all(20),
            children: [
              if (signedIn.isNotEmpty)
                FormSection(
                  title: 'Connected accounts',
                  children: [
                    for (final credential in signedIn)
                      _CredentialCard(
                        credential: credential,
                        onReconnect: _busy
                            ? null
                            : () => _signIn(credential.provider),
                        onForget: _busy
                            ? null
                            : () => widget.credentials.forget(
                                credential.provider,
                              ),
                      ),
                  ],
                ),
              FormSection(
                title: 'API access',
                children: [
                  for (final provider in ProviderKind.values.where(
                    (p) => p.needsApiKey,
                  ))
                    Padding(
                      padding: const EdgeInsets.symmetric(vertical: 6),
                      child: Row(
                        children: [
                          Icon(
                            Icons.key_outlined,
                            size: 22,
                            color: scheme.secondary,
                          ),
                          const SizedBox(width: 12),
                          Expanded(
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Text(
                                  provider.label,
                                  style: const TextStyle(
                                    fontWeight: FontWeight.w600,
                                  ),
                                ),
                                const SizedBox(height: 3),
                                Text(
                                  widget.credentials.isSignedIn(provider)
                                      ? 'Key saved for all your bots'
                                      : 'One key for all your bots',
                                  style: TextStyle(
                                    fontSize: 12,
                                    color: scheme.onSurfaceVariant,
                                  ),
                                ),
                              ],
                            ),
                          ),
                          TextButton(
                            onPressed: _busy ? null : () => _apiKey(provider),
                            child: Text(
                              widget.credentials.isSignedIn(provider)
                                  ? 'Manage'
                                  : 'Add key',
                            ),
                          ),
                        ],
                      ),
                    ),
                ],
              ),
              if (kDeviceSignInProviders.any(
                (p) => !widget.credentials.isSignedIn(p),
              ))
                FormSection(
                  title: 'Connect a subscription',
                  children: [
                    Text(
                      'Sign in once. Choose the account when you add a bot.',
                      style: TextStyle(
                        fontSize: 13,
                        height: 1.5,
                        color: scheme.onSurfaceVariant,
                      ),
                    ),
                    const SizedBox(height: 14),
                    for (final provider in kDeviceSignInProviders.where(
                      (p) => !widget.credentials.isSignedIn(p),
                    ))
                      Padding(
                        padding: const EdgeInsets.only(bottom: 8),
                        child: _SignInButton(
                          provider: provider,
                          enabled: !_busy,
                          onPressed: () => _signIn(provider),
                        ),
                      ),
                  ],
                ),
              if (_error != null) ...[
                const SizedBox(height: 12),
                _Banner(
                  text: _error!,
                  tone: scheme.error,
                  icon: Icons.error_outline,
                ),
              ],
              if (_success != null) ...[
                const SizedBox(height: 12),
                _Banner(
                  text: _success!,
                  tone: Colors.green,
                  icon: Icons.check_circle_outline,
                ),
              ],
              const SizedBox(height: 20),
              Text(
                'Claude subscriptions are available in the Linux build through the system CLI.',
                style: TextStyle(
                  fontSize: 12,
                  height: 1.45,
                  color: scheme.onSurfaceVariant,
                ),
              ),
              const SizedBox(height: 18),
              const Divider(),
              TextButton.icon(
                onPressed: () => setState(() => _showPairing = !_showPairing),
                icon: Icon(
                  _showPairing ? Icons.expand_less : Icons.expand_more,
                  size: 18,
                ),
                label: const Text('Bring a sign-in from a computer instead'),
              ),
              if (_showPairing) ...[
                const SizedBox(height: 6),
                Text(
                  'On a computer that has the CLIs signed in, run this and '
                  'paste everything it prints. It contains live tokens, so '
                  'treat it like a password.',
                  style: TextStyle(
                    fontSize: 12.5,
                    height: 1.4,
                    color: scheme.onSurfaceVariant,
                  ),
                ),
                const SizedBox(height: 10),
                Container(
                  width: double.infinity,
                  padding: const EdgeInsets.all(13),
                  decoration: BoxDecoration(
                    color: scheme.surfaceContainerHighest,
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: const SelectionArea(
                    child: Text(
                      'orgbot pair',
                      style: TextStyle(fontFamily: 'monospace', fontSize: 13),
                    ),
                  ),
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: _bundle,
                  maxLines: 4,
                  minLines: 2,
                  style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
                  decoration: const InputDecoration(
                    labelText: 'Pairing bundle',
                    hintText: '{"version":1,...}',
                    border: OutlineInputBorder(),
                    alignLabelWithHint: true,
                  ),
                ),
                const SizedBox(height: 12),
                Row(
                  children: [
                    Expanded(
                      child: FilledButton.tonalIcon(
                        onPressed: _busy ? null : _import,
                        icon: const Icon(Icons.download_outlined),
                        label: const Text('Import'),
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
              ],
              if (widget.credentials.all.isNotEmpty) ...[
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

/// The code to type, and where to type it.
class _DeviceCodePanel extends StatelessWidget {
  const _DeviceCodePanel({required this.flow, required this.onCancel});

  final DeviceSignIn flow;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final challenge = flow.challenge;
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        const SizedBox(height: 12),
        Center(
          child: Text(
            'Approve ' + flow.provider.label,
            style: Theme.of(context).textTheme.titleLarge,
          ),
        ),
        const SizedBox(height: 8),
        Center(
          child: Text(
            'Open the page below and enter this code.',
            style: TextStyle(fontSize: 13, color: scheme.onSurfaceVariant),
          ),
        ),
        const SizedBox(height: 26),
        Center(
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 26, vertical: 18),
            decoration: BoxDecoration(
              color: scheme.surfaceContainerHighest,
              borderRadius: BorderRadius.circular(14),
              border: Border.all(color: scheme.outlineVariant),
            ),
            child: SelectionArea(
              child: Text(
                challenge.userCode,
                style: const TextStyle(
                  fontFamily: 'monospace',
                  fontSize: 30,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 3,
                ),
              ),
            ),
          ),
        ),
        const SizedBox(height: 14),
        Center(
          child: TextButton.icon(
            onPressed: () =>
                Clipboard.setData(ClipboardData(text: challenge.userCode)),
            icon: const Icon(Icons.copy, size: 16),
            label: const Text('Copy code'),
          ),
        ),
        const SizedBox(height: 22),
        FilledButton.icon(
          onPressed: () => launchUrl(
            Uri.parse(challenge.bestUri),
            mode: LaunchMode.externalApplication,
          ),
          icon: const Icon(Icons.open_in_new),
          label: const Text('Open the sign-in page'),
        ),
        const SizedBox(height: 12),
        Center(
          child: SelectionArea(
            child: Text(
              challenge.verificationUri,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
            ),
          ),
        ),
        const SizedBox(height: 30),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const SizedBox(
              width: 15,
              height: 15,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(width: 11),
            Text(
              'Waiting for you to approve…',
              style: TextStyle(fontSize: 13, color: scheme.onSurfaceVariant),
            ),
          ],
        ),
        const SizedBox(height: 26),
        Center(
          child: TextButton(onPressed: onCancel, child: const Text('Cancel')),
        ),
      ],
    );
  }
}

/// One provider's sign-in button.
class _SignInButton extends StatelessWidget {
  const _SignInButton({
    required this.provider,
    required this.enabled,
    required this.onPressed,
  });

  final ProviderKind provider;
  final bool enabled;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final label = 'Sign in with ${provider.label}';
    return SizedBox(
      width: double.infinity,
      child: OutlinedButton.icon(
        onPressed: enabled ? onPressed : null,
        icon: const Icon(Icons.login, size: 18),
        label: Text(label),
      ),
    );
  }
}

class _CredentialCard extends StatelessWidget {
  const _CredentialCard({
    required this.credential,
    required this.onForget,
    required this.onReconnect,
  });

  final ProviderCredential credential;
  final VoidCallback? onForget, onReconnect;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final lapsed = credential.needsRenewal && !credential.canRenew;
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 12),
      decoration: BoxDecoration(
        border: Border(
          bottom: BorderSide(
            color: lapsed ? scheme.error : scheme.outlineVariant,
            width: 0.5,
          ),
        ),
      ),
      child: Row(
        children: [
          Icon(
            lapsed ? Icons.error_outline : Icons.verified_user_outlined,
            size: 20,
            color: lapsed ? scheme.error : scheme.tertiary,
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  credential.provider.label,
                  style: const TextStyle(
                    fontWeight: FontWeight.w700,
                    fontSize: 14,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  lapsed
                      ? 'Expired — sign in again'
                      : [
                          if (credential.plan != null) credential.plan!,
                          if (credential.account != null) credential.account!,
                          if (credential.models.isNotEmpty)
                            '${credential.models.length} models',
                          if (credential.plan == null &&
                              credential.account == null &&
                              credential.models.isEmpty)
                            'Signed in',
                        ].join(' · '),
                  style: TextStyle(
                    fontSize: 11.5,
                    color: lapsed ? scheme.error : scheme.onSurfaceVariant,
                  ),
                ),
              ],
            ),
          ),
          PopupMenuButton<String>(
            tooltip: '${credential.provider.label} account options',
            enabled: onForget != null,
            icon: const Icon(Icons.more_horiz, size: 20),
            onSelected: (action) =>
                action == 'reconnect' ? onReconnect?.call() : onForget?.call(),
            itemBuilder: (_) => const [
              PopupMenuItem(value: 'reconnect', child: Text('Reconnect')),
              PopupMenuItem(value: 'forget', child: Text('Sign out')),
            ],
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
            child: Text(
              text,
              style: TextStyle(fontSize: 12.5, height: 1.35, color: tone),
            ),
          ),
        ],
      ),
    );
  }
}
