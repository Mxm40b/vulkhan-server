use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

#[derive(PartialEq)]
pub enum PacketType {
    Join,
    Leave,
    Update,
    ShareToken,
}

#[derive(Clone)]
pub enum SendToWhom {
    ToAll(Vec<u8>),
    ToOne(u32, Vec<u8>),
}

impl PacketType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(PacketType::Join),
            1 => Some(PacketType::Leave),
            2 => Some(PacketType::Update),
            3 => Some(PacketType::ShareToken),
            _ => None,
        }
    }
    pub fn to_u8(&self) -> u8 {
        match self {
            Self::Join => 0,
            Self::Leave => 1,
            Self::Update => 2,
            Self::ShareToken => 3,
        }
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, KnownLayout, Immutable)]
pub struct Packet {
    packet_type: u8,
    pub id: u32,
    // can be both token and user id depending on context
    position: [f32; 3],
    orientation: [f32; 4],
}

impl Packet {
    // convert a packet to usable data
    // because they are not interchangeable in rust
    pub fn to_data(self, id: u32) -> Option<PlayerData> {
        if PacketType::from_u8(self.packet_type)
            .expect("server does not handle unvalid packet type for now")
            == PacketType::Update
        {
            return Some(PlayerData {
                id,
                orientation: glam::Quat::from_xyzw(
                    self.orientation[0],
                    self.orientation[1],
                    self.orientation[2],
                    self.orientation[3],
                ),
                position: glam::Vec3::new(self.position[0], self.position[1], self.position[2]),
            });
        };
        None
    }
}

pub struct PlayerData {
    position: glam::Vec3,
    orientation: glam::Quat,
    pub id: u32,
}

impl PlayerData {
    pub fn new(id: u32) -> PlayerData {
        PlayerData {
            id,
            // starting position:
            position: glam::Vec3::new(0f32, 0f32, 0f32),
            orientation: glam::quat(0f32, 0f32, 0f32, 0f32),
        }
    }
    // to make sure the packet sent back has the same structure
    // as the one received, we insert everything into the
    // correctly laid-out in memory
    // Packet type then take it as bytes
    pub fn to_packet_bytes(&self, packet_type: PacketType) -> Vec<u8> {
        Packet {
            packet_type: packet_type.to_u8(),
            id: self.id,
            position: self.position.to_array(),
            orientation: self.orientation.to_array(),
        }
        .as_bytes()
        .to_vec()
    }
    pub fn with_id(&self, id: u32) -> Self {
        PlayerData { id, ..*self }
    }
}

pub fn generate_token() -> u32 {
    // todo: use better random algorithm for generating tokens than this
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("why wouldn't it calculate the duration since epoch??")
        .as_secs() as u32
    // using as u32 removes the upper bits
}
