using System;
using System.IO;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace Ludex.Playnite
{
    /// <summary>
    /// Configuration read from <c>ludex.config.json</c> next to the extension
    /// assembly. Keeping it a file (instead of the Playnite settings UI) keeps
    /// this extension dependency-light and easy to inspect.
    /// </summary>
    public sealed class LudexConfig
    {
        /// <summary>Base URL of the Ludex server, without a trailing slash.</summary>
        public string ServerUrl { get; set; } = "http://localhost:8081";

        /// <summary>API key (<c>ldx_…</c>) or a session JWT with read scope.</summary>
        public string ApiKey { get; set; } = string.Empty;

        /// <summary>
        /// Template used for the Play action. Defaults to the Ludex game page;
        /// set it to <c>ludex://play/{gameId}</c> when the desktop app registers
        /// the scheme.
        /// </summary>
        public string LaunchUrlTemplate { get; set; } = "{serverUrl}/game/{gameId}";

        /// <summary>Include games that have no downloadable version.</summary>
        public bool IncludeUnavailable { get; set; }

        public string NormalizedServerUrl
        {
            get
            {
                var url = (ServerUrl ?? string.Empty).Trim();
                return url.TrimEnd('/');
            }
        }

        public string BuildLaunchUrl(long gameId)
        {
            var template = string.IsNullOrWhiteSpace(LaunchUrlTemplate)
                ? "{serverUrl}/game/{gameId}"
                : LaunchUrlTemplate;

            return template
                .Replace("{serverUrl}", NormalizedServerUrl)
                .Replace("{gameId}", gameId.ToString(System.Globalization.CultureInfo.InvariantCulture));
        }

        public static LudexConfig Load(string extensionDirectory)
        {
            var path = Path.Combine(extensionDirectory, "ludex.config.json");
            try
            {
                if (!File.Exists(path))
                {
                    var config = new LudexConfig();
                    File.WriteAllText(path, JsonConvert.SerializeObject(config, Formatting.Indented));
                    return config;
                }

                var parsed = JsonConvert.DeserializeObject<LudexConfig>(File.ReadAllText(path));
                return parsed ?? new LudexConfig();
            }
            catch (Exception)
            {
                // A broken config must not make Playnite fail to start.
                return new LudexConfig();
            }
        }

        public JObject ToJson()
        {
            return JObject.FromObject(this);
        }
    }
}
