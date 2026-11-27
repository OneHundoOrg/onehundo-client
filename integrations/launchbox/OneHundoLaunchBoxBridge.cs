// Drop-in C# plugin scaffold for LaunchBox / Big Box (`IGameLaunchingPlugin` / `ISystemEventsPlugin`).
// Compiles against `Unbroken.LaunchBox.Plugins.dll` when copied into `LaunchBox/Plugins/OneHundo/`.
// Fail-open over loopback `127.0.0.1:19850` with a 1500ms timeout so game launches never stall.

using System;
using System.Net.Http;
using System.Text;
using System.Threading.Tasks;

namespace OneHundo.LaunchBoxBridge
{
    public sealed class OneHundoHttpClient
    {
        private static readonly HttpClient Http = new HttpClient
        {
            Timeout = TimeSpan.FromMilliseconds(1500)
        };

        private readonly string _baseUrl;

        public OneHundoHttpClient(string baseUrl = "http://127.0.0.1:19850")
        {
            _baseUrl = baseUrl.TrimEnd('/');
        }

        public async Task ReportGameAsync(
            string launchBoxGameId,
            string executablePath,
            string rootFolder,
            string saveDir,
            bool active)
        {
            try
            {
                var payload = "{"
                    + "\"launcher\":\"launchbox\","
                    + "\"app_id\":" + JsonStr(launchBoxGameId) + ","
                    + "\"executable\":" + JsonStr(executablePath) + ","
                    + "\"install_dir\":" + JsonStr(rootFolder) + ","
                    + "\"save_dir\":" + JsonStr(saveDir) + ","
                    + "\"os\":\"windows\","
                    + "\"active\":" + (active ? "true" : "false")
                    + "}";

                using (var content = new StringContent(payload, Encoding.UTF8, "application/json"))
                {
                    await Http.PostAsync(_baseUrl + "/v1/launchers/report", content).ConfigureAwait(false);
                }
            }
            catch
            {
                // Fail-open: never interrupt LaunchBox or Big Box UI/launch pipeline.
            }
        }

        public async Task<string> QueryStatusJsonAsync(string canonicalGameId = null)
        {
            try
            {
                var url = _baseUrl + "/v1/launchers/status";
                if (!string.IsNullOrEmpty(canonicalGameId))
                {
                    url += "?canonical_game_id=" + Uri.EscapeDataString(canonicalGameId);
                }
                return await Http.GetStringAsync(url).ConfigureAwait(false);
            }
            catch
            {
                return "{\"games\":[]}";
            }
        }

        private static string JsonStr(string value)
        {
            if (string.IsNullOrEmpty(value)) return "null";
            return "\"" + value
                .Replace("\\", "\\\\")
                .Replace("\"", "\\\"")
                .Replace("\r", "\\r")
                .Replace("\n", "\\n") + "\"";
        }
    }
}
