#include "Core.h"
#include <winrt/base.h>
#include <iostream>
#include <stdexcept>
using namespace connecty;
void Check(bool value, char const *message)
{
    if (!value)
        throw std::runtime_error(message);
}
int main()
{
    try
    {
        winrt::init_apartment(winrt::apartment_type::multi_threaded);
        Check(Quote(L"path with spaces\\") == L"\"path with spaces\\\\\"", "Win32 executable quoting");
        Check(Decode("\xff").size() > 0, "Malformed UTF-8 is safe");
        Profile p;
        p.host = L"server";
        Check(Validate(p).empty(), "Valid profile");
        p.remotePort = 0;
        Check(!Validate(p).empty(), "Invalid port rejected");
        p.remotePort = 22023;
        p.bindAddress = L"0.0.0.0";
        Check(!Validate(p).empty(), "Public listener rejected");
        Check(StateText(State::Verified) == L"Connected", "Compact verified label");
        std::cout << "Frontend core checks passed\n";
        return 0;
    }
    catch (std::exception const &e)
    {
        std::cerr << e.what() << '\n';
        return 1;
    }
}
