# Installed Vibe must resolve every inference/admin provider through these
# schema methods. Keep the selected key in this child only; never use keyring.
import os as _gah_os
_gah_selected_key = _gah_os.environ.get("MISTRAL_API_KEY")
if not _gah_selected_key:
    raise RuntimeError("The selected Vibe credential is unavailable.")

from vibe.core.config.vibe_schema import VibeConfigSchema as _GahVibeConfig
from vibe.utils import api_keys as _gah_api_keys
_gah_os.environ["MISTRAL_API_KEY"] = _gah_selected_key

def _gah_provider(config, provider):
    if (provider is None or provider.name != "mistral"
        or provider.api_base.rstrip("/") != "https://api.mistral.ai/v1"
        or provider.api_key_env_var != "MISTRAL_API_KEY"
        or getattr(provider, "extra_headers", {})
        or getattr(provider, "backend", "generic") not in ("mistral", "generic")
        or getattr(provider, "api_style", "openai") != "openai"
        or config.vibe_base_url.rstrip("/") != "https://chat.mistral.ai"):
        raise RuntimeError("Vibe configuration overrides the selected credential provider.")
    return provider

_gah_original_provider = _GahVibeConfig.get_provider_for_model
_gah_original_mistral = _GahVibeConfig.get_mistral_provider

def _gah_model_provider(self, model):
    return _gah_provider(self, _gah_original_provider(self, model))

def _gah_mistral_provider(self):
    return _gah_provider(self, _gah_original_mistral(self))

def _gah_key_with_origin(env_key):
    if env_key != "MISTRAL_API_KEY":
        raise RuntimeError("Vibe configuration overrides the selected credential provider.")
    return (_gah_selected_key, _gah_api_keys.ApiKeyOrigin(
        _gah_api_keys.ApiKeySource.ENVIRONMENT, env_key))

_GahVibeConfig.get_provider_for_model = _gah_model_provider
_GahVibeConfig.get_mistral_provider = _gah_mistral_provider
_gah_api_keys.resolve_api_key_with_origin = _gah_key_with_origin
