#include "Core.h"
#include <shlobj.h>
#include <shellapi.h>
#include <sddl.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <algorithm>
#include <chrono>
#include <stdexcept>
using namespace winrt;
using namespace Windows::Data::Json;
namespace connecty
{
namespace
{
constexpr DWORD MaxFrame = 1024 * 1024;
std::wstring Sid(HANDLE process)
{
    HANDLE token{};
    check_bool(OpenProcessToken(process, TOKEN_QUERY, &token));
    DWORD size = 0;
    GetTokenInformation(token, TokenUser, nullptr, 0, &size);
    std::vector<BYTE> data(size);
    bool ok = GetTokenInformation(token, TokenUser, data.data(), size, &size) != FALSE;
    CloseHandle(token);
    check_bool(ok);
    LPWSTR text{};
    check_bool(ConvertSidToStringSidW(reinterpret_cast<TOKEN_USER *>(data.data())->User.Sid, &text));
    std::wstring result = text;
    LocalFree(text);
    return result;
}
std::filesystem::path AgentExe()
{
    wchar_t path[32768]{};
    GetModuleFileNameW(nullptr, path, 32768);
    return std::filesystem::path(path).parent_path() / L"Connecty.Agent.exe";
}
bool TrustedServer(HANDLE pipe)
{
    ULONG pid{};
    if (!GetNamedPipeServerProcessId(pipe, &pid))
        return false;
    HANDLE process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
    if (!process)
        return false;
    bool trusted = false;
    try
    {
        wchar_t path[32768]{};
        DWORD size = 32768;
        trusted = QueryFullProcessImageNameW(process, 0, path, &size) &&
                  _wcsicmp(path, AgentExe().c_str()) == 0 && Sid(process) == Sid(GetCurrentProcess());
    }
    catch (...)
    {
    }
    CloseHandle(process);
    return trusted;
}
void LaunchAgent()
{
    auto exe = AgentExe();
    std::wstring command = Quote(exe.wstring()) + L" --watchdog";
    STARTUPINFOW start{sizeof(start)};
    PROCESS_INFORMATION process{};
    check_bool(CreateProcessW(exe.c_str(), command.data(), nullptr, nullptr, FALSE, CREATE_NO_WINDOW, nullptr,
                              exe.parent_path().c_str(), &start, &process));
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
}
bool Transfer(HANDLE pipe, HANDLE stop, void *data, DWORD count, bool write)
{
    BYTE *cursor = static_cast<BYTE *>(data);
    while (count)
    {
        OVERLAPPED operation{};
        operation.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        if (!operation.hEvent)
            return false;
        DWORD done = 0;
        BOOL ok = write ? WriteFile(pipe, cursor, count, &done, &operation)
                        : ReadFile(pipe, cursor, count, &done, &operation);
        if (!ok && GetLastError() == ERROR_IO_PENDING)
        {
            HANDLE events[] = {operation.hEvent, stop};
            DWORD result = WaitForMultipleObjects(2, events, FALSE, write ? 5000 : INFINITE);
            if (result == WAIT_OBJECT_0)
                ok = GetOverlappedResult(pipe, &operation, &done, FALSE);
            else
            {
                CancelIoEx(pipe, &operation);
                GetOverlappedResult(pipe, &operation, &done, TRUE);
                ok = FALSE;
            }
        }
        CloseHandle(operation.hEvent);
        if (!ok || !done)
            return false;
        cursor += done;
        count -= done;
    }
    return true;
}
Profile ParseProfile(JsonObject const &o)
{
    Profile p;
    p.id = o.GetNamedString(L"id");
    p.name = o.GetNamedString(L"name");
    p.host = o.GetNamedString(L"host");
    p.hostname = o.GetNamedString(L"hostname", L"");
    p.user = o.GetNamedString(L"user", L"");
    p.identityFile = o.GetNamedString(L"identityFile", L"");
    p.knownHosts = o.GetNamedString(L"knownHosts", L"");
    p.direction = o.GetNamedString(L"direction", L"reverse");
    p.bindAddress = o.GetNamedString(L"bindAddress", L"127.0.0.1");
    p.target = o.GetNamedString(L"target", L"127.0.0.1");
    p.probe = o.GetNamedString(L"probe", L"");
    p.port = int(o.GetNamedNumber(L"port", 0));
    p.remotePort = int(o.GetNamedNumber(L"remotePort", 22023));
    p.targetPort = int(o.GetNamedNumber(L"targetPort", 22));
    p.retry = o.GetNamedBoolean(L"retry", true);
    p.autoConnect = o.GetNamedBoolean(L"autoConnect", false);
    return p;
}
JsonObject Encode(Profile const &p)
{
    JsonObject o;
    auto str = [&](wchar_t const *key, std::wstring const &value) {
        o.SetNamedValue(key, JsonValue::CreateStringValue(value));
    };
    str(L"id", p.id);
    str(L"name", p.name);
    str(L"host", p.host);
    str(L"hostname", p.hostname);
    str(L"user", p.user);
    str(L"identityFile", p.identityFile);
    str(L"knownHosts", p.knownHosts);
    str(L"direction", p.direction);
    str(L"bindAddress", p.bindAddress);
    str(L"target", p.target);
    str(L"probe", p.probe);
    o.SetNamedValue(L"port", JsonValue::CreateNumberValue(p.port));
    o.SetNamedValue(L"remotePort", JsonValue::CreateNumberValue(p.remotePort));
    o.SetNamedValue(L"targetPort", JsonValue::CreateNumberValue(p.targetPort));
    o.SetNamedValue(L"retry", JsonValue::CreateBooleanValue(p.retry));
    o.SetNamedValue(L"autoConnect", JsonValue::CreateBooleanValue(p.autoConnect));
    return o;
}
State ParseState(hstring const &s)
{
    if (s == L"connecting")
        return State::Connecting;
    if (s == L"forwarding")
        return State::Forwarding;
    if (s == L"verified")
        return State::Verified;
    if (s == L"retrying")
        return State::Retrying;
    if (s == L"suspended")
        return State::Suspended;
    if (s == L"error")
        return State::Error;
    return State::Stopped;
}
} // namespace
std::wstring Decode(std::string const &bytes)
{
    if (bytes.empty())
        return {};
    int n = MultiByteToWideChar(CP_UTF8, 0, bytes.data(), int(bytes.size()), nullptr, 0);
    std::wstring text(n, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, bytes.data(), int(bytes.size()), text.data(), n);
    return text;
}
std::wstring Quote(std::wstring const &value)
{
    std::wstring out = L"\"";
    size_t slashes = 0;
    for (wchar_t c : value)
    {
        if (c == L'\\')
        {
            ++slashes;
            continue;
        }
        if (c == L'\"')
        {
            out.append(slashes * 2 + 1, L'\\');
            out += c;
        }
        else
        {
            out.append(slashes, L'\\');
            out += c;
        }
        slashes = 0;
    }
    out.append(slashes * 2, L'\\');
    return out + L'\"';
}
std::wstring Validate(Profile const &p)
{
    if (p.name.empty() || p.host.empty())
        return L"Add a name and SSH host.";
    if (p.remotePort < 1 || p.remotePort > 65535 || p.targetPort < 1 || p.targetPort > 65535 || p.port < 0 ||
        p.port > 65535)
        return L"Ports must be 1–65535.";
    if (p.bindAddress != L"127.0.0.1" && p.bindAddress != L"::1")
        return L"Use a loopback listening address.";
    return {};
}
std::wstring NewId()
{
    GUID guid{};
    check_hresult(CoCreateGuid(&guid));
    wchar_t text[40]{};
    StringFromGUID2(guid, text, 40);
    return text;
}
std::filesystem::path DataPath()
{
    PWSTR path{};
    check_hresult(SHGetKnownFolderPath(FOLDERID_LocalAppData, 0, nullptr, &path));
    auto dir = std::filesystem::path(path) / L"Connecty";
    CoTaskMemFree(path);
    std::filesystem::create_directories(dir);
    return dir;
}
AgentClient &AgentClient::Instance()
{
    static AgentClient instance;
    return instance;
}
AgentClient::~AgentClient()
{
    if (stop_)
    {
        SetEvent(stop_);
        {
            std::lock_guard lock(writer_);
            if (pipe_ != INVALID_HANDLE_VALUE)
                CancelIoEx(pipe_, nullptr);
        }
        if (reader_.joinable())
            reader_.join();
        CloseHandle(stop_);
    }
}
void AgentClient::Start()
{
    if (reader_.joinable())
        return;
    stop_ = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    check_bool(stop_ != nullptr);
    reader_ = std::thread([this] {
        init_apartment(apartment_type::multi_threaded);
        Run();
        uninit_apartment();
    });
    std::unique_lock lock(mutex_);
    changed_.wait_for(lock, std::chrono::seconds(8), [&] { return view_.online || !view_.error.empty(); });
    if (!view_.online)
        throw std::runtime_error(
            to_string(view_.error.empty() ? L"Background agent did not start" : view_.error));
}
void AgentClient::OnChanged(std::function<void()> callback)
{
    std::lock_guard lock(mutex_);
    callback_ = std::move(callback);
}
AgentView AgentClient::Read() const
{
    std::lock_guard lock(mutex_);
    return view_;
}
void AgentClient::Notify()
{
    std::function<void()> callback;
    {
        std::lock_guard lock(mutex_);
        callback = callback_;
    }
    changed_.notify_all();
    if (callback)
        callback();
}
void AgentClient::Receive(std::wstring const &message)
{
    auto envelope = JsonObject::Parse(message);
    auto object = envelope.GetNamedObject(L"snapshot", nullptr);
    if (object)
    {
        AgentView view;
        view.online = true;
        view.pid = unsigned(object.GetNamedNumber(L"agentPid", 0));
        view.startup = object.GetNamedBoolean(L"startup", false);
        auto error = object.GetNamedValue(L"error", JsonValue::CreateNullValue());
        if (error.ValueType() == JsonValueType::Object)
            view.error = error.GetObject().GetNamedString(L"message", L"");
        for (auto const &value : object.GetNamedArray(L"profiles"))
            view.profiles.push_back(ParseProfile(value.GetObject()));
        for (auto const &item : object.GetNamedObject(L"connections"))
        {
            auto o = item.Value().GetObject();
            Snapshot s;
            s.state = ParseState(o.GetNamedString(L"state", L"stopped"));
            s.detail = o.GetNamedString(L"detail", L"");
            s.retries = unsigned(o.GetNamedNumber(L"retries", 0));
            s.since = uint64_t(o.GetNamedNumber(L"since", 0));
            auto verified = o.GetNamedValue(L"verifiedAt", JsonValue::CreateNullValue());
            if (verified.ValueType() == JsonValueType::Number)
                s.verifiedAt = uint64_t(verified.GetNumber());
            auto e = o.GetNamedValue(L"error", JsonValue::CreateNullValue());
            if (e.ValueType() == JsonValueType::Object)
                s.errorCode = e.GetObject().GetNamedString(L"code", L"");
            for (auto const &row : o.GetNamedArray(L"logs"))
            {
                auto entry = row.GetObject();
                LogEntry log{uint64_t(entry.GetNamedNumber(L"time")), entry.GetNamedString(L"level").c_str(),
                             entry.GetNamedString(L"code").c_str(), entry.GetNamedString(L"message").c_str()};
                s.logs += std::to_wstring(log.time) + L" " + log.level + L" " + log.code + L" " + log.message + L"\n";
                s.entries.push_back(std::move(log));
            }
            view.connections.emplace(item.Key(), std::move(s));
        }
        {
            std::lock_guard lock(mutex_);
            view_ = std::move(view);
        }
    }
    if (envelope.GetNamedString(L"event", L"") == L"reply")
    {
        std::lock_guard lock(mutex_);
        replies_[uint64_t(envelope.GetNamedNumber(L"requestId"))] = message;
        if (replies_.size() > 32)
            replies_.erase(replies_.begin());
    }
    Notify();
}
void AgentClient::Run()
{
    auto name = L"\\\\.\\pipe\\Connecty.Agent." + Sid(GetCurrentProcess()) + L".v1";
    ULONGLONG nextLaunch = 0;
    DWORD delay = 250;
    while (!stopAgent_ && WaitForSingleObject(stop_, 0) != WAIT_OBJECT_0)
    {
        HANDLE pipe =
            CreateFileW(name.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING,
                        FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, nullptr);
        if (pipe == INVALID_HANDLE_VALUE)
        {
            if (GetTickCount64() >= nextLaunch)
            {
                nextLaunch = GetTickCount64() + 3000;
                try
                {
                    LaunchAgent();
                }
                catch (hresult_error const &e)
                {
                    {
                        std::lock_guard lock(mutex_);
                        view_.error = e.message();
                    }
                    Notify();
                }
            }
            WaitNamedPipeW(name.c_str(), std::min(delay, 1000ul));
            if (WaitForSingleObject(stop_, delay) == WAIT_OBJECT_0)
                break;
            delay = std::min(delay * 2, 30000ul);
            continue;
        }
        if (!TrustedServer(pipe))
        {
            CloseHandle(pipe);
            {
                std::lock_guard lock(mutex_);
                view_.error = L"Agent identity verification failed";
            }
            Notify();
            break;
        }
        delay = 250;
        nextLaunch = 0;
        {
            std::lock_guard lock(writer_);
            pipe_ = pipe;
        }
        try
        {
            while (WaitForSingleObject(stop_, 0) != WAIT_OBJECT_0)
            {
                DWORD size = 0;
                if (!Transfer(pipe, stop_, &size, 4, false) || !size || size > MaxFrame)
                    break;
                std::string bytes(size, '\0');
                if (!Transfer(pipe, stop_, bytes.data(), size, false))
                    break;
                Receive(Decode(bytes));
            }
        }
        catch (...)
        {
        }
        {
            std::lock_guard lock(writer_);
            pipe_ = INVALID_HANDLE_VALUE;
            CloseHandle(pipe);
        }
        {
            std::lock_guard lock(mutex_);
            view_.online = false;
            view_.error = L"Background agent disconnected";
        }
        Notify();
        if (WaitForSingleObject(stop_, delay) == WAIT_OBJECT_0)
            break;
    }
}
JsonObject AgentClient::Command(std::wstring const &op, std::wstring const &id, JsonObject extra)
{
    if (op == L"shutdown")
        stopAgent_ = true;
    auto request = extra;
    uint64_t sequence = next_++;
    request.SetNamedValue(L"version", JsonValue::CreateNumberValue(1));
    request.SetNamedValue(L"requestId", JsonValue::CreateNumberValue(double(sequence)));
    request.SetNamedValue(L"op", JsonValue::CreateStringValue(op));
    request.SetNamedValue(L"id", JsonValue::CreateStringValue(id));
    auto bytes = to_string(request.Stringify());
    struct Erase
    {
        std::string &value;
        ~Erase()
        {
            if (!value.empty())
                SecureZeroMemory(value.data(), value.size());
        }
    } erase{bytes};
    DWORD size = DWORD(bytes.size());
    if (size > MaxFrame)
        throw std::runtime_error("Request too large");
    {
        std::lock_guard lock(writer_);
        if (pipe_ == INVALID_HANDLE_VALUE || !Transfer(pipe_, stop_, &size, 4, true) ||
            !Transfer(pipe_, stop_, bytes.data(), size, true))
            throw std::runtime_error("Agent unavailable");
    }
    std::unique_lock lock(mutex_);
    if (!changed_.wait_for(lock, std::chrono::seconds(15),
                           [&] { return replies_.contains(sequence) || !view_.online; }))
        throw std::runtime_error("Agent request timed out");
    auto found = replies_.find(sequence);
    if (found == replies_.end())
        throw std::runtime_error("Agent disconnected");
    auto response = JsonObject::Parse(found->second);
    replies_.erase(found);
    if (!response.GetNamedBoolean(L"ok", false))
    {
        auto e = response.GetNamedObject(L"error");
        throw std::runtime_error(to_string(e.GetNamedString(L"message", L"Agent rejected request")));
    }
    return response;
}
std::vector<Profile> LoadProfiles()
{
    AgentClient::Instance().Start();
    auto view = AgentClient::Instance().Read();
    if (!view.error.empty())
        throw std::runtime_error(to_string(view.error));
    return view.profiles;
}
void SaveProfiles(std::vector<Profile> const &profiles)
{
    JsonArray array;
    for (auto const &p : profiles)
        array.Append(Encode(p));
    JsonObject extra;
    extra.SetNamedValue(L"profiles", array);
    AgentClient::Instance().Command(L"configure", L"", extra);
}
void SetStartup(bool enabled)
{
    JsonObject extra;
    extra.SetNamedValue(L"enabled", JsonValue::CreateBooleanValue(enabled));
    AgentClient::Instance().Command(L"startup", L"", extra);
}
bool StartupEnabled()
{
    return AgentClient::Instance().Read().startup;
}
std::wstring StateText(State s)
{
    switch (s)
    {
    case State::Connecting:
        return L"Connecting";
    case State::Forwarding:
        return L"Connected";
    case State::Verified:
        return L"Connected";
    case State::Retrying:
        return L"Reconnecting";
    case State::Suspended:
        return L"Sleeping";
    case State::Error:
        return L"Needs attention";
    default:
        return L"Disconnected";
    }
}
Snapshot Tunnel::Read() const
{
    auto view = AgentClient::Instance().Read();
    if (!view.online)
    {
        Snapshot s;
        s.state = State::Error;
        s.detail = L"Agent unavailable";
        return s;
    }
    auto it = view.connections.find(profile_.id);
    return it == view.connections.end() ? Snapshot{} : it->second;
}
} // namespace connecty
