#pragma once
#include <windows.h>
#include <string>
#include <vector>
#include <map>
#include <mutex>
#include <thread>
#include <atomic>
#include <condition_variable>
#include <functional>
#include <filesystem>
#include <winrt/Windows.Data.Json.h>
namespace connecty
{
struct Profile
{
    std::wstring id, name = L"New connection", host, hostname, user, identityFile, knownHosts;
    std::wstring direction = L"reverse", bindAddress = L"127.0.0.1", target = L"127.0.0.1", probe;
    int remotePort = 22023, targetPort = 22, port = 0;
    bool retry = true, autoConnect = false;
};
std::wstring Decode(std::string const &);
std::wstring Quote(std::wstring const &);
std::wstring Validate(Profile const &);
std::wstring NewId();
std::filesystem::path DataPath();
std::vector<Profile> LoadProfiles();
void SaveProfiles(std::vector<Profile> const &);
void SetStartup(bool);
bool StartupEnabled();
enum class State
{
    Stopped,
    Connecting,
    Forwarding,
    Verified,
    Retrying,
    Suspended,
    Error
};
std::wstring StateText(State);
struct LogEntry
{
    uint64_t time{};
    std::wstring level, code, message;
};
struct Snapshot
{
    State state = State::Stopped;
    std::wstring detail = L"Ready", logs, errorCode;
    std::vector<LogEntry> entries;
    unsigned retries = 0;
    uint64_t since = 0, verifiedAt = 0;
};
struct AgentView
{
    bool online = false, startup = false;
    unsigned pid = 0;
    std::wstring error;
    std::vector<Profile> profiles;
    std::map<std::wstring, Snapshot> connections;
};
class AgentClient
{
    mutable std::mutex mutex_;
    std::mutex writer_;
    std::condition_variable changed_;
    AgentView view_;
    std::map<uint64_t, std::wstring> replies_;
    std::function<void()> callback_;
    std::thread reader_;
    HANDLE stop_ = nullptr, pipe_ = INVALID_HANDLE_VALUE;
    std::atomic<uint64_t> next_{1};
    std::atomic<bool> stopAgent_{false};
    void Run();
    void Receive(std::wstring const &);
    void Notify();

  public:
    static AgentClient &Instance();
    ~AgentClient();
    void Start();
    void OnChanged(std::function<void()> callback);
    AgentView Read() const;
    winrt::Windows::Data::Json::JsonObject Command(std::wstring const &op, std::wstring const &id = L"",
                 winrt::Windows::Data::Json::JsonObject extra = {});
};
class Tunnel
{
    Profile profile_;

  public:
    explicit Tunnel(Profile p) : profile_(std::move(p)) {}
    // The agent owns connection lifetime. Destroying UI proxies never disconnects.
    void Start()
    {
        AgentClient::Instance().Command(L"connect", profile_.id);
    }
    void Stop()
    {
        AgentClient::Instance().Command(L"disconnect", profile_.id);
    }
    void Restart()
    {
        AgentClient::Instance().Command(L"reconnect", profile_.id);
    }
    Snapshot Read() const;
    Profile const &Config() const
    {
        return profile_;
    }
};
} // namespace connecty
